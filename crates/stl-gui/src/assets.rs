//! Pictures: decoded on worker threads (a 2560 px JPEG takes a moment), then uploaded as textures on the UI thread.
//! The background comes in two versions: sharp for the window, and small, blurred and darkened for the glass cards.

use eframe::egui::{self, vec2, ColorImage, TextureHandle, TextureOptions, Vec2};
use image::imageops::{self, FilterType};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

pub struct Tex {
    /// the picture, or for an animation the frame to show where it cannot play
    pub handle: TextureHandle,
    pub size: Vec2,
    /// the frames of an animated GIF with how long each shows, in milliseconds
    pub frames: Vec<(TextureHandle, f32)>,
}

impl Tex {
    /// The texture to show `secs` seconds into the animation (the picture itself when it is not one).
    pub fn at(&self, secs: f64) -> &TextureHandle {
        let total: f32 = self.frames.iter().map(|f| f.1).sum();
        if self.frames.is_empty() || total <= 0.0 {
            return &self.handle;
        }
        let mut t = ((secs * 1000.0) as f32) % total;
        for (h, ms) in &self.frames {
            if t < *ms {
                return h;
            }
            t -= ms;
        }
        &self.handle
    }

    pub fn animated(&self) -> bool {
        self.frames.len() > 1
    }
}

enum Loaded {
    Image(String, ColorImage),
    Animation(String, Vec<(ColorImage, f32)>),
    Background(PathBuf, ColorImage, ColorImage),
    Failed(String),
}

pub struct Assets {
    ctx: egui::Context,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
    textures: HashMap<String, Tex>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    background_wanted: Option<PathBuf>,
    background_loaded: Option<PathBuf>,
    pub background: Option<(Tex, Tex)>,
}

/// The frames of a GIF, thinned to at most `max_frames` (the delay of the dropped ones is added to the kept ones), each shrunk to `max_w`.
fn decode_gif(path: &Path, max_w: u32, max_frames: usize) -> Option<Vec<(ColorImage, f32)>> {
    use image::AnimationDecoder;
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let decoder = image::codecs::gif::GifDecoder::new(file).ok()?;
    let frames = decoder.into_frames().collect_frames().ok()?;
    if frames.len() < 2 {
        return None;
    }
    let step = frames.len().div_ceil(max_frames).max(1);
    let mut out = Vec::new();
    for chunk in frames.chunks(step) {
        let ms: f32 = chunk.iter().map(|f| { let (n, d) = f.delay().numer_denom_ms(); n as f32 / d.max(1) as f32 }).sum();
        let img = shrink_to(chunk[0].buffer().clone(), max_w);
        out.push((to_color_image(&img), ms.max(20.0)));
    }
    Some(out)
}

fn decode(path: &Path) -> Result<image::RgbaImage, String> {
    let reader = image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    Ok(reader.decode().map_err(|e| e.to_string())?.to_rgba8())
}

fn to_color_image(img: &image::RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

fn shrink_to(img: image::RgbaImage, max_w: u32) -> image::RgbaImage {
    if img.width() <= max_w {
        return img;
    }
    let h = (img.height() as f32 * max_w as f32 / img.width() as f32).round().max(1.0) as u32;
    imageops::resize(&img, max_w, h, FilterType::Triangle)
}

/// The blurred, darkened copy the glass shows.
fn frost(img: &image::RgbaImage) -> image::RgbaImage {
    let small = shrink_to(img.clone(), 320);
    let mut blurred = imageops::blur(&small, 5.0);
    for px in blurred.pixels_mut() {
        for c in 0..3 {
            px.0[c] = (px.0[c] as f32 * 0.58) as u8;
        }
        px.0[3] = 255;
    }
    blurred
}

impl Assets {
    pub fn new(ctx: &egui::Context) -> Assets {
        let (tx, rx) = channel();
        Assets { ctx: ctx.clone(), tx, rx, textures: HashMap::new(), pending: HashSet::new(), failed: HashSet::new(), background_wanted: None, background_loaded: None, background: None }
    }

    /// Takes in what the workers have finished; call once per frame.
    pub fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Loaded::Image(key, img) => {
                    self.pending.remove(&key);
                    let size = vec2(img.size[0] as f32, img.size[1] as f32);
                    let handle = self.ctx.load_texture(&key, img, TextureOptions::LINEAR);
                    self.textures.insert(key, Tex { handle, size, frames: Vec::new() });
                }
                Loaded::Animation(key, frames) => {
                    self.pending.remove(&key);
                    let size = vec2(frames[0].0.size[0] as f32, frames[0].0.size[1] as f32);
                    let handles: Vec<(TextureHandle, f32)> = frames.into_iter().enumerate().map(|(i, (img, ms))| (self.ctx.load_texture(format!("{key}#{i}"), img, TextureOptions::LINEAR), ms)).collect();
                    // where nothing plays, show the middle of the loop: the first frame of an ad is often a bare start
                    let handle = handles[handles.len() / 2].0.clone();
                    self.textures.insert(key, Tex { handle, size, frames: handles });
                }
                Loaded::Background(path, sharp, blurred) => {
                    let mk = |name: &str, img: ColorImage| {
                        let size = vec2(img.size[0] as f32, img.size[1] as f32);
                        Tex { handle: self.ctx.load_texture(name, img, TextureOptions::LINEAR), size, frames: Vec::new() }
                    };
                    let a = mk("background", sharp);
                    let b = mk("background-frost", blurred);
                    if self.background_wanted.as_deref() == Some(&path) {
                        self.background = Some((a, b));
                    }
                    self.background_loaded = Some(path);
                }
                Loaded::Failed(key) => {
                    self.pending.remove(&key);
                    self.failed.insert(key);
                }
            }
        }
    }

    /// Asks for the window's background (None = no picture). Loads in the background; the old one stays until the new one is ready.
    pub fn set_background(&mut self, path: Option<PathBuf>) {
        if self.background_wanted == path {
            return;
        }
        self.background_wanted = path.clone();
        match path {
            None => self.background = None,
            Some(p) => {
                if self.background_loaded.as_ref() == Some(&p) {
                    return;
                }
                let tx = self.tx.clone();
                let ctx = self.ctx.clone();
                std::thread::spawn(move || {
                    let msg = match decode(&p) {
                        Ok(img) => {
                            let blurred = frost(&img);
                            Loaded::Background(p, to_color_image(&shrink_to(img, 2560)), to_color_image(&blurred))
                        }
                        Err(_) => Loaded::Failed(String::new()),
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                });
            }
        }
    }

    /// A picture by file, once it has been decoded (None until then, and for ever if it cannot be read).
    pub fn image(&mut self, path: &Path, max_width: u32) -> Option<&Tex> {
        let key = path.to_string_lossy().to_string();
        if self.textures.contains_key(&key) {
            return self.textures.get(&key);
        }
        if !self.pending.contains(&key) && !self.failed.contains(&key) {
            self.pending.insert(key.clone());
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();
            let p = path.to_path_buf();
            std::thread::spawn(move || {
                let animation = if p.extension().map_or(false, |e| e.eq_ignore_ascii_case("gif")) { decode_gif(&p, max_width, 48) } else { None };
                let msg = match animation {
                    Some(frames) => Loaded::Animation(key, frames),
                    None => match decode(&p) {
                        Ok(img) => Loaded::Image(key, to_color_image(&shrink_to(img, max_width))),
                        Err(_) => Loaded::Failed(key),
                    },
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            });
        }
        None
    }

    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
}
