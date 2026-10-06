//! The news cards the official launcher shows on its home page (promotions of DLC and other games, announcements): a public, anonymous feed of
//! "content cards" per game. We read what the official launcher has cached, and can fetch the same public feed ourselves; pictures are kept in
//! our own cache. Nothing about the user is sent and no impression is reported; a card only opens its link in the browser when it is clicked.

use crate::{net, paths, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const FEED_URL: &str = "https://api.paradox-interactive.com/communication/braze/contentcards";

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    /// `main`, `secondary-1`, … as the feed names the slot
    pub slot: String,
    pub id: String,
    pub title: String,
    pub text: String,
    pub image_url: Option<String>,
    /// the picture on disk, when it has been fetched
    pub image: Option<PathBuf>,
    pub link: Option<String>,
}

fn slot_rank(slot: &str) -> (u8, String) {
    (if slot == "main" { 0 } else { 1 }, slot.to_string())
}

/// The cards of a feed document, the main slot first. `language` picks the text (falls back to English, then to whatever there is).
pub fn parse_feed(json: &str, language: &str) -> Vec<Card> {
    let Ok(doc) = serde_json::from_str::<Value>(json.trim_start_matches('\u{feff}')) else { return Vec::new() };
    let Some(slots) = doc.as_object() else { return Vec::new() };
    let mut names: Vec<&String> = slots.keys().collect();
    names.sort_by_key(|n| slot_rank(n));
    let pick = |v: &Value| -> String {
        v.get(language).or_else(|| v.get("en")).and_then(|x| x.as_str()).or_else(|| v.as_object().and_then(|o| o.values().find_map(|x| x.as_str()))).unwrap_or("").to_string()
    };
    let mut out = Vec::new();
    for name in names {
        let Some(groups) = slots[name].get("contentGroups").and_then(|g| g.as_array()) else { continue };
        for g in groups {
            let Some(items) = g.get("contentItems").and_then(|i| i.as_array()) else { continue };
            for item in items {
                let c = &item["content"];
                let image_url = c["image"]["src"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
                if image_url.is_none() && c["link"]["href"].as_str().is_none() {
                    continue;
                }
                out.push(Card {
                    slot: name.clone(),
                    id: item["id"].as_str().unwrap_or("").to_string(),
                    title: pick(&c["title"]),
                    text: pick(&c["text"]),
                    image_url,
                    image: None,
                    link: c["link"]["href"].as_str().filter(|s| s.starts_with("http")).map(str::to_string),
                });
            }
        }
    }
    // the same card can sit in two slots with one picture: keep the first
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert((c.image_url.clone(), c.link.clone())));
    out
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(paths::app_data_dir()?.join("cache").join("news"))
}

fn image_name(url: &str) -> String {
    let h = Sha256::digest(url.as_bytes());
    let stem: String = h.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let ext = url.split('?').next().and_then(|u| u.rsplit('.').next()).filter(|e| matches!(*e, "png" | "jpg" | "jpeg" | "gif" | "webp")).unwrap_or("img");
    format!("{stem}.{ext}")
}

/// The feed we last fetched, with the pictures that are in our cache.
pub fn load_cached(language: &str) -> Vec<Card> {
    let Ok(dir) = cache_dir() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(dir.join("feed.json")) else { return Vec::new() };
    let mut cards = parse_feed(&text, language);
    for c in &mut cards {
        if let Some(u) = &c.image_url {
            let p = dir.join(image_name(u));
            if p.is_file() {
                c.image = Some(p);
            }
        }
    }
    cards
}

/// The feed the official launcher cached for this game (`Documents\…\.launcher-cache\anonymous-news-feed-cache`), with its own picture files.
pub fn load_official_cache(data_dir: &Path, language: &str) -> Vec<Card> {
    let base = data_dir.join(".launcher-cache").join("anonymous-news-feed-cache");
    for name in ["news-feed-definition-steam", "news-feed-definition"] {
        let Ok(text) = std::fs::read_to_string(base.join(name).join(name)) else { continue };
        let mut cards = parse_feed(&text, language);
        // the cached document says where each picture was saved (`cacheSrc`: file:///C:\…)
        if let Ok(doc) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
            let mut local: std::collections::HashMap<String, PathBuf> = std::collections::HashMap::new();
            fn walk(v: &Value, local: &mut std::collections::HashMap<String, PathBuf>) {
                match v {
                    Value::Object(o) => {
                        if let (Some(src), Some(cache)) = (o.get("src").and_then(|x| x.as_str()), o.get("cacheSrc").and_then(|x| x.as_str())) {
                            let p = cache.trim_start_matches("file://").trim_start_matches('/');
                            local.insert(src.to_string(), PathBuf::from(p.replace('/', "\\")));
                        }
                        o.values().for_each(|x| walk(x, local));
                    }
                    Value::Array(a) => a.iter().for_each(|x| walk(x, local)),
                    _ => {}
                }
            }
            walk(&doc, &mut local);
            for c in &mut cards {
                if let Some(p) = c.image_url.as_ref().and_then(|u| local.get(u)) {
                    if p.is_file() {
                        c.image = Some(p.clone());
                    }
                }
            }
        }
        if !cards.is_empty() {
            return cards;
        }
    }
    Vec::new()
}

/// Fetches the public feed for a game id (`stellaris`), saves it and its pictures in our cache and returns the cards.
pub fn refresh(game_id: &str, platform: &str, language: &str) -> Result<Vec<Card>> {
    let url = format!("{FEED_URL}/{game_id}?distributionPlatform={platform}");
    let body = net::http_get(&url, 15_000, 4 << 20)?;
    let text = String::from_utf8(body).context("the feed is not text")?;
    let mut cards = parse_feed(&text, language);
    let dir = cache_dir()?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("feed.json"), &text)?;
    for c in &mut cards {
        let Some(u) = c.image_url.clone() else { continue };
        let p = dir.join(image_name(&u));
        if !p.is_file() {
            match net::http_get(&u, 20_000, 12 << 20) {
                Ok(bytes) => {
                    let _ = std::fs::write(&p, bytes);
                }
                Err(_) => continue,
            }
        }
        if p.is_file() {
            c.image = Some(p);
        }
    }
    Ok(cards)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"{
      "secondary-1": { "slotType": "secondary-1", "contentGroups": [ { "contentItems": [
        { "id": "b", "content": { "title": {"en": "Patch notes", "de": "Patchnotizen"}, "text": {"en": "x"}, "image": {"src": "https://img/b.jpg?1"}, "link": {"href": "https://forum/x"} } } ] } ] },
      "main": { "slotType": "main", "contentGroups": [ { "contentItems": [
        { "id": "a", "content": { "title": {"en": ""}, "text": {"en": ""}, "image": {"src": "https://img/a.png"}, "link": {"href": "https://shop/y"} } },
        { "id": "dup", "content": { "title": {"en": ""}, "text": {"en": ""}, "image": {"src": "https://img/a.png"}, "link": {"href": "https://shop/y"} } },
        { "id": "empty", "content": { "title": {"en": ""}, "text": {"en": ""}, "image": {"src": ""}, "link": {} } } ] } ] }
    }"#;

    #[test]
    fn parses_slots_in_order_without_duplicates() {
        let c = parse_feed(FEED, "en");
        assert_eq!(c.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"], "main first, duplicate and empty dropped");
        assert_eq!(c[1].title, "Patch notes");
        assert_eq!(c[0].link.as_deref(), Some("https://shop/y"));
        assert_eq!(parse_feed(FEED, "de")[1].title, "Patchnotizen");
        assert_eq!(parse_feed(FEED, "fr")[1].title, "Patch notes", "falls back to English");
        assert!(parse_feed("not json", "en").is_empty());
    }

    #[test]
    fn names_images_by_url() {
        let a = image_name("https://img/a.png?3");
        assert!(a.ends_with(".png"));
        assert_ne!(a, image_name("https://img/b.png?3"));
        assert!(image_name("https://x/noext").ends_with(".img"));
    }
}
