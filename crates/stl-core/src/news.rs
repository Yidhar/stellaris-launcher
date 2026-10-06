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
    /// how long the card shows before the next one of its slot, in milliseconds
    pub delay_ms: u64,
}

fn slot_rank(slot: &str) -> (u8, String) {
    (if slot == "main" { 0 } else { 1 }, slot.to_string())
}

/// The cards of a feed document, the way the Paradox Launcher picks them: for each slot (`main`, `secondary-1`, `secondary-2`, in that order),
/// the content groups that are visible now (`settings.visible.from/until`) and whose `settings.filter` (`owns` / `or` / `and`) holds for the
/// installed DLC are kept, one of them is drawn by `settings.weight`, and its items are that slot's cards, shown in turn for their `delay`.
/// Items repeat on purpose (a card listed four times shows four times as often). `language` picks the text (English, then anything).
pub fn parse_feed(json: &str, language: &str) -> Vec<Card> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let mut seed = (now as u64) ^ 0x9E37_79B9_7F4A_7C15;
    parse_feed_with(json, language, now, &[], &mut || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    })
}

fn parse_time_ms(s: &str) -> Option<i64> {
    // `2024-01-01T00:00:00Z` (or with a fraction); the date and time are enough
    let s = s.trim();
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.trim_end_matches('Z');
    let time = time.split(['+', '.']).next()?;
    let mut t = time.split(':').map(|x| x.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next().flatten().unwrap_or(0));
    // days from the civil date (Howard Hinnant's algorithm)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(((days * 86400) + hh * 3600 + mm * 60 + ss) * 1000)
}

fn visible(settings: &Value, now_ms: i64) -> bool {
    let v = &settings["visible"];
    if v.is_null() {
        return true;
    }
    let from = v["from"].as_str().and_then(parse_time_ms);
    let until = v["until"].as_str().and_then(parse_time_ms);
    from.map_or(true, |f| now_ms >= f) && until.map_or(true, |u| now_ms <= u)
}

fn filter_holds(filter: &Value, owned: &[String]) -> bool {
    if filter.is_null() {
        return true;
    }
    fn holds(f: &Value, owned: &[String]) -> bool {
        if let Some(o) = f.get("owns") {
            let o = o.as_str().map(str::to_string).unwrap_or_else(|| o.to_string()).to_lowercase();
            return owned.iter().any(|x| x.to_lowercase() == o);
        }
        if let Some(a) = f.get("or").and_then(|x| x.as_array()) {
            return a.iter().any(|x| holds(x, owned));
        }
        if let Some(a) = f.get("and").and_then(|x| x.as_array()) {
            return a.iter().all(|x| holds(x, owned));
        }
        false
    }
    holds(filter, owned)
}

/// `parse_feed` with the clock, the owned DLC (their ids as the feed names them) and the random draw given.
pub fn parse_feed_with(json: &str, language: &str, now_ms: i64, owned: &[String], random: &mut dyn FnMut() -> f64) -> Vec<Card> {
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
        let live: Vec<&Value> = groups.iter().filter(|g| visible(&g["settings"], now_ms) && filter_holds(&g["settings"]["filter"], owned)).collect();
        if live.is_empty() {
            continue;
        }
        // one group, drawn by weight (the first when no group has a weight)
        let total: f64 = live.iter().map(|g| g["settings"]["weight"].as_f64().unwrap_or(0.0)).sum();
        let mut r = random() * total;
        let chosen = live.iter().find(|g| {
            let w = g["settings"]["weight"].as_f64().unwrap_or(0.0);
            if r < w {
                true
            } else {
                r -= w;
                false
            }
        }).copied().unwrap_or(live[0]);
        let items: Vec<&Value> = match &chosen["contentItems"] {
            Value::Array(a) => a.iter().collect(),
            Value::Object(o) => o.values().collect(),
            _ => Vec::new(),
        };
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
                delay_ms: item["delay"].as_u64().or_else(|| c["delay"].as_u64()).unwrap_or(5000).max(1000),
            });
        }
    }
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
    fn picks_one_visible_group_per_slot_like_the_official_launcher() {
        let feed = r#"{
          "secondary-2": { "contentGroups": [
            { "settings": {"weight": 1}, "contentItems": [
              { "id": "tf", "delay": 4000, "content": { "image": {"src": "https://img/tf.png"}, "link": {"href": "https://x/tf"} } },
              { "id": "tf", "content": { "image": {"src": "https://img/tf.png"}, "link": {"href": "https://x/tf"} } } ] } ] },
          "main": { "contentGroups": [
            { "settings": {"weight": 1, "visible": {"from": "2020-01-01T00:00:00Z", "until": "2021-01-01T00:00:00Z"}}, "contentItems": [
              { "id": "old", "content": { "image": {"src": "https://img/old.png"}, "link": {"href": "https://x/old"} } } ] },
            { "settings": {"weight": 1, "filter": {"owns": "dlc_nomads"}}, "contentItems": [
              { "id": "owners", "content": { "image": {"src": "https://img/o.png"}, "link": {"href": "https://x/o"} } } ] },
            { "settings": {"weight": 3}, "contentItems": [
              { "id": "a", "content": { "title": {"en": "A", "de": "Ä"}, "image": {"src": "https://img/a.png"}, "link": {"href": "https://x/a"} } } ] } ] }
        }"#;
        let now = parse_time_ms("2026-10-06T12:00:00Z").unwrap();
        let ids = |owned: &[String], r: f64| parse_feed_with(feed, "en", now, owned, &mut || r).into_iter().map(|c| c.id).collect::<Vec<_>>();
        // the expired group never shows; without the DLC the filtered one does not either
        assert_eq!(ids(&[], 0.0), vec!["a", "tf", "tf"], "main first; repeated items stay");
        assert_eq!(ids(&[], 0.99), vec!["a", "tf", "tf"]);
        // owning it, the draw by weight (1 : 3) picks it for small numbers
        let owned = vec!["DLC_NOMADS".to_string()];
        assert_eq!(ids(&owned, 0.1)[0], "owners");
        assert_eq!(ids(&owned, 0.9)[0], "a");
        let c = parse_feed_with(feed, "de", now, &[], &mut || 0.0);
        assert_eq!(c[0].title, "Ä");
        assert_eq!(c[1].delay_ms, 4000);
        assert_eq!(c[2].delay_ms, 5000, "5 s when the item says nothing");
        assert!(parse_feed("not json", "en").is_empty());
    }

    #[test]
    fn reads_iso_times() {
        assert_eq!(parse_time_ms("1970-01-02T00:00:00Z"), Some(86_400_000));
        assert_eq!(parse_time_ms("2024-01-01T00:00:00.000Z"), Some(1_704_067_200_000));
        assert_eq!(parse_time_ms("nonsense"), None);
    }

    #[test]
    fn names_images_by_url() {
        let a = image_name("https://img/a.png?3");
        assert!(a.ends_with(".png"));
        assert_ne!(a, image_name("https://img/b.png?3"));
        assert!(image_name("https://x/noext").ends_with(".img"));
    }
}
