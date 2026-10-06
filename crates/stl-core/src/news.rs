//! The news cards the official launcher shows on its home page (patch notes, dev diaries, promotions). It has two sources:
//! - signed in, the cards Braze sends to the account (the official launcher keeps them in its Chromium Local Storage, `ab.storage.cc.<key>`);
//!   we read that copy from disk, so a user who signs in there sees the same news here;
//! - otherwise a public, anonymous feed per game, which we fetch ourselves (and can read from the official launcher's cache).
//! Pictures are kept in our own cache. Nothing about the user is sent and no impression is reported; a card only opens its link when clicked.

use crate::{net, paths, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The Braze app key the official launcher's cards are stored under.
const BRAZE_KEY: &str = "0381b29f-827d-4e24-9906-ad99933faa11";
/// Launcher starts we assume for the cards aimed at a number of them (a returning player).
const ASSUMED_LAUNCHES: f64 = 100.0;

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

fn ms_of(v: &Value) -> Option<i64> {
    v.as_str().and_then(parse_time_ms).or_else(|| v.as_i64().map(|x| if x < 100_000_000_000 { x * 1000 } else { x }))
}

/// The account's cards (the stored Braze list, minified keys: `i` picture, `u` link, `e` extras, `ca`/`ea` created/expires, `p` pinned, `r` removed)
/// picked the way the official launcher does: the card is for this game (`extras.game`) and platform (`extras.distributionPlatforms`), its
/// launch-count target holds, and it names a known section; then each slot in turn (main, secondary-1, secondary-2) takes up to 8 cards that
/// list it in `extras.sections` and no earlier slot took. Each shows for `extras.delay` (4 s when unset).
pub fn parse_braze(json: &str, game_id: &str, platform: &str, now_ms: i64) -> Vec<Card> {
    let Ok(doc) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    let Some(list) = doc.get("v").unwrap_or(&doc).as_array() else { return Vec::new() };
    let split = |v: &Value| -> Vec<String> { v.as_str().map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default() };
    let num = |v: &Value| -> Option<f64> { v.as_str().and_then(|s| s.trim().parse::<f64>().ok()).or_else(|| v.as_f64()).filter(|x| *x != 0.0) };
    const SLOTS: [&str; 3] = ["main", "secondary-1", "secondary-2"];
    let mut cards: Vec<&Value> = list
        .iter()
        .filter(|c| {
            let e = &c["e"];
            let link = c["u"].as_str().unwrap_or("");
            if !e.is_object() || link.is_empty() || c["r"].as_bool() == Some(true) {
                return false;
            }
            if ms_of(&c["ea"]).is_some_and(|t| t < now_ms) {
                return false;
            }
            let n = ASSUMED_LAUNCHES;
            let launches_ok = match num(&e["numberOfLauncherStarts"]) {
                Some(exact) => exact == n,
                None => !(num(&e["minNumberOfLauncherStarts"]).is_some_and(|m| n < m) || num(&e["maxNumberOfLauncherStarts"]).is_some_and(|m| n > m)),
            };
            split(&e["game"]).iter().any(|g| g == game_id)
                && split(&e["distributionPlatforms"]).iter().any(|p| p == platform)
                && launches_ok
                && split(&e["sections"]).iter().any(|s| SLOTS.contains(&s.as_str()) || s == "onboarding" || s == "onboarding-main")
        })
        .collect();
    // as the SDK lists them: pinned first, then the newest
    cards.sort_by_key(|c| (c["p"].as_bool() != Some(true), std::cmp::Reverse(ms_of(&c["ca"]).unwrap_or(0))));
    let mut used = std::collections::HashSet::new();
    let mut out = Vec::new();
    for slot in SLOTS {
        let mut n = 0;
        for c in &cards {
            let id = c["id"].as_str().unwrap_or("").to_string();
            if n >= 8 || used.contains(&id) || !split(&c["e"]["sections"]).iter().any(|s| s == slot) {
                continue;
            }
            n += 1;
            used.insert(id.clone());
            out.push(Card {
                slot: slot.to_string(),
                id,
                title: c["tt"].as_str().unwrap_or("").to_string(),
                text: c["ds"].as_str().unwrap_or("").to_string(),
                image_url: c["i"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
                image: None,
                link: c["u"].as_str().filter(|s| s.starts_with("http")).map(str::to_string),
                delay_ms: num(&c["e"]["delay"]).map(|d| d as u64).unwrap_or(4000).max(1000),
            });
        }
    }
    out
}

/// The cards the official launcher received for the signed-in account, read from its Local Storage; empty when it never signed in.
pub fn load_account_cards(game_id: &str, platform: &str) -> Vec<Card> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else { return Vec::new() };
    let dir = PathBuf::from(local).join("Paradox Interactive").join("launcher-v2").join("chromium-data").join("Local Storage").join("leveldb");
    let key = format!("ab.storage.cc.{BRAZE_KEY}");
    let values = crate::leveldb::read(&dir, key.as_bytes());
    let Some(text) = values.iter().find(|(k, _)| k.ends_with(key.as_bytes())).and_then(|(_, v)| crate::leveldb::chromium_string(v)) else { return Vec::new() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    parse_braze(&text, game_id, platform, now)
}

fn attach_cached_images(cards: &mut [Card]) {
    let Ok(dir) = cache_dir() else { return };
    for c in cards {
        if let Some(u) = &c.image_url {
            let p = dir.join(image_name(u));
            if p.is_file() {
                c.image = Some(p);
            }
        }
    }
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

/// What can be shown without the network: the account's cards, else the feed we last fetched; with the pictures that are in our cache.
pub fn load_cached(game_id: &str, platform: &str, language: &str) -> Vec<Card> {
    let mut cards = load_account_cards(game_id, platform);
    if cards.is_empty() {
        let Ok(dir) = cache_dir() else { return Vec::new() };
        let Ok(text) = std::fs::read_to_string(dir.join("feed.json")) else { return Vec::new() };
        cards = parse_feed(&text, language);
    }
    attach_cached_images(&mut cards);
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

/// The account's cards when the official launcher has them, else the public feed for a game id (`stellaris`), fetched and saved; with their
/// pictures downloaded into our cache.
pub fn refresh(game_id: &str, platform: &str, language: &str) -> Result<Vec<Card>> {
    let dir = cache_dir()?;
    std::fs::create_dir_all(&dir)?;
    let mut cards = load_account_cards(game_id, platform);
    if cards.is_empty() {
        let url = format!("{FEED_URL}/{game_id}?distributionPlatform={platform}");
        let body = net::http_get(&url, 15_000, 4 << 20)?;
        let text = String::from_utf8(body).context("the feed is not text")?;
        cards = parse_feed(&text, language);
        std::fs::write(dir.join("feed.json"), &text)?;
    }
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
    fn picks_account_cards_like_the_official_launcher() {
        let cards = r#"{"v":[
          {"id":"old","i":"https://img/o.jpg","u":"https://x/o","ca":"2026-09-01T00:00:00Z","ea":"2026-09-02T00:00:00Z","e":{"game":"stellaris","sections":"main","distributionPlatforms":"steam"}},
          {"id":"m","i":"https://img/m.jpg","u":"https://x/m","ca":"2026-09-22T00:00:00Z","ea":"2026-10-20T00:00:00Z","e":{"game":"stellaris","sections":"main","distributionPlatforms":"steam,pdx"}},
          {"id":"dd","i":"https://img/dd.jpg","u":"https://x/dd","ca":"2026-10-06T00:00:00Z","ea":"2026-10-08T00:00:00Z","e":{"game":"stellaris","delay":"6000","sections":"secondary-1","distributionPlatforms":"steam","minNumberOfLauncherStarts":"2"}},
          {"id":"both","i":"https://img/b.jpg","u":"https://x/b","ca":"2026-10-05T00:00:00Z","e":{"game":"ck3, stellaris","sections":"secondary-1,secondary-2","distributionPlatforms":"steam"}},
          {"id":"new","i":"https://img/n.jpg","u":"https://x/n","ca":"2026-10-05T00:00:00Z","e":{"game":"stellaris","sections":"secondary-2","distributionPlatforms":"steam","maxNumberOfLauncherStarts":"3"}},
          {"id":"gog","i":"https://img/g.jpg","u":"https://x/g","ca":"2026-10-05T00:00:00Z","e":{"game":"stellaris","sections":"main","distributionPlatforms":"gog"}},
          {"id":"ck","i":"https://img/c.jpg","u":"https://x/c","ca":"2026-10-05T00:00:00Z","e":{"game":"ck3","sections":"main","distributionPlatforms":"steam"}},
          {"id":"nolink","i":"https://img/l.jpg","u":"","ca":"2026-10-05T00:00:00Z","e":{"game":"stellaris","sections":"main","distributionPlatforms":"steam"}}
        ]}"#;
        let now = parse_time_ms("2026-10-06T12:00:00Z").unwrap();
        let c = parse_braze(cards, "stellaris", "steam", now);
        let got: Vec<(&str, &str)> = c.iter().map(|c| (c.slot.as_str(), c.id.as_str())).collect();
        // expired, other platforms, other games, cards for new players and cards without a link are left out; a card fills one slot only
        assert_eq!(got, vec![("main", "m"), ("secondary-1", "dd"), ("secondary-1", "both")]);
        assert_eq!(c[1].delay_ms, 6000);
        assert_eq!(c[0].delay_ms, 4000);
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
