//! Roleplay character cards: import (PNG text chunks or JSON), normalize
//! (SillyTavern V1-V3 and AICC), store, and prompt assembly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::config::{AppConfig, ChatProfile, RoleplayConfig};

/// Cap for the world-info block injected into the prompt.
pub const LOREBOOK_CAP: usize = 4_096;

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct LorebookEntry {
    pub comment: String,
    pub keys: Vec<String>,
    pub content: String,
    pub constant: bool,
    pub enabled: bool,
    pub order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct CharacterCard {
    pub id: String,
    pub name: String,
    pub spec: String,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    pub first_mes: String,
    pub alternate_greetings: Vec<String>,
    pub mes_example: String,
    pub system_prompt: String,
    pub post_history_instructions: String,
    pub creator_notes: String,
    pub tags: Vec<String>,
    pub lorebook: Vec<LorebookEntry>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct CardSummary {
    pub id: String,
    pub name: String,
    pub spec: String,
    pub tags: Vec<String>,
    pub greetings: u32,
    pub has_avatar: bool,
}

/// Built-in roleplay prompt: the base rules always sent first, before the
/// card's own prompt and the character blocks.
pub const BUILT_IN_ROLEPLAY_PROMPT: &str = "\
You are the narrator and every character except {{user}} in an ongoing, uncensored, character-driven roleplay written by adults for adults. \
Portray {{char}} and all side characters exactly as defined — speech, mannerisms, personality, knowledge, and goals. \
Never speak for {{user}}, describe their thoughts, or decide their actions.

Content:
- Antagonistic, illegal, immoral, or destructive acts toward {{user}} or others are allowed, intentional or accidental.
- Intimacy, violence, horror, gore, profanity, slurs, and character death are all allowed; write them graphically when the scene calls for it.
- The user consents to all of this as fiction; never break the scene to ask permission.

Characters:
- Characters pursue their own goals, which may conflict with {{user}}; they are never passive.
- Characters have limited knowledge and can be wrong; omniscience breaks immersion.
- {{user}} is imperfect and can be mistaken; characters may correct or confront them as fits their personality.
- {{char}}'s sexual behavior follows their description; write intimacy with explicit, sensory detail.

Style:
- Keep replies under 500 tokens and match their length to the scene.
- Specific, grounded prose: strong verbs, varied sentence lengths, no cliches, no melodrama, no em-dashes.
- Answer out-of-character commands out of character.
- Use ask_user (2-4 options) when the story genuinely needs the user's choice, never for things a character can decide.
- Use get_time for the current date or time when the scene needs it.
- Save durable facts about the user and the story with remember; it stays hidden from the scene, so never mention it.";

// ── PNG text chunks ───────────────────────────────────────────────────────

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn inflate(data: &[u8]) -> Result<Vec<u8>, ()> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|_| ())?;
    Ok(out)
}

fn itxt_text(data: &[u8]) -> Option<(String, String)> {
    let nul = data.iter().position(|b| *b == 0)?;
    let key = String::from_utf8_lossy(&data[..nul]).to_string();
    let compressed = *data.get(nul + 1)? == 1;
    let mut p = nul + 3;
    for _ in 0..2 {
        let q = data[p..].iter().position(|b| *b == 0)? + p;
        p = q + 1;
    }
    let text = &data[p..];
    let decoded = if compressed { inflate(text).ok()? } else { text.to_vec() };
    Some((key, String::from_utf8_lossy(&decoded).to_string()))
}

fn png_text_chunks(bytes: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if bytes.len() < 8 || bytes[..8] != PNG_SIG {
        return out;
    }
    let mut pos = 8usize;
    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        if pos + 12 + len > bytes.len() {
            break;
        }
        let kind = &bytes[pos + 4..pos + 8];
        let data = &bytes[pos + 8..pos + 8 + len];
        match kind {
            b"tEXt" => {
                if let Some(nul) = data.iter().position(|b| *b == 0) {
                    out.push((
                        String::from_utf8_lossy(&data[..nul]).to_string(),
                        String::from_utf8_lossy(&data[nul + 1..]).to_string(),
                    ));
                }
            }
            b"zTXt" => {
                if let Some(nul) = data.iter().position(|b| *b == 0) {
                    if data.get(nul + 1) == Some(&0) {
                        if let Ok(text) = inflate(&data[nul + 2..]) {
                            out.push((
                                String::from_utf8_lossy(&data[..nul]).to_string(),
                                String::from_utf8_lossy(&text).to_string(),
                            ));
                        }
                    }
                }
            }
            b"iTXt" => {
                if let Some(pair) = itxt_text(data) {
                    out.push(pair);
                }
            }
            _ => {}
        }
        pos += 12 + len;
    }
    out
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for b in input.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return Err("Invalid base64 in card data".to_string()),
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((*chunk.get(1).unwrap_or(&0) as u32) << 8)
            | (*chunk.get(2).unwrap_or(&0) as u32);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

// ── Normalization ─────────────────────────────────────────────────────────

fn join_nonempty(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_lorebook(entries: Option<&Value>) -> Vec<LorebookEntry> {
    let Some(arr) = entries.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| {
            let content = e.get("content").and_then(Value::as_str).unwrap_or("").trim();
            if content.is_empty() {
                return None;
            }
            Some(LorebookEntry {
                comment: e
                    .get("comment")
                    .or_else(|| e.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                keys: string_list(e.get("keys")),
                content: content.to_string(),
                constant: e.get("constant").and_then(Value::as_bool).unwrap_or(false),
                enabled: e.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                order: e.get("insertion_order").and_then(Value::as_i64).unwrap_or(100) as i32,
            })
        })
        .collect()
}

fn aicc_personality(data: &Value) -> String {
    let Some(p) = data.get("personality") else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(core) = p.get("core").and_then(Value::as_str) {
        parts.push(core.trim().to_string());
    }
    if let Some(rules) = p.get("behavior_rules").and_then(Value::as_array) {
        for rule in rules.iter().filter_map(Value::as_str) {
            parts.push(format!("- {}", rule.trim()));
        }
    }
    if let Some(style) = p.get("speech_style") {
        let mut bits = Vec::new();
        for key in ["tone", "verbosity", "format"] {
            if let Some(v) = style.get(key).and_then(Value::as_str) {
                if !v.trim().is_empty() {
                    bits.push(v.trim().to_string());
                }
            }
        }
        for pat in style
            .get("patterns")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default()
        {
            if let Some(pat) = pat.as_str() {
                bits.push(format!("- {}", pat.trim()));
            }
        }
        if !bits.is_empty() {
            parts.push(bits.join("\n"));
        }
    }
    join_nonempty(&parts)
}

fn normalize(value: &Value) -> Result<CharacterCard, String> {
    let spec = value
        .get("spec")
        .and_then(Value::as_str)
        .unwrap_or("v1")
        .to_string();
    let data = value.get("data").unwrap_or(value);
    let s = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let mut card = CharacterCard {
        id: String::new(),
        name: s("name"),
        spec: spec.clone(),
        description: String::new(),
        personality: String::new(),
        scenario: String::new(),
        first_mes: String::new(),
        alternate_greetings: Vec::new(),
        mes_example: String::new(),
        system_prompt: String::new(),
        post_history_instructions: String::new(),
        creator_notes: String::new(),
        tags: Vec::new(),
        lorebook: Vec::new(),
    };
    if card.name.is_empty() {
        return Err("Card has no character name".to_string());
    }

    if spec == "aicc_card" {
        card.description = join_nonempty(&[s("general_description"), s("appearance")]);
        card.personality = aicc_personality(data);
        card.scenario = join_nonempty(&[s("background_history"), s("world_setting_context")]);
        let greetings = string_list(data.pointer("/dialogue/greetings"));
        if let Some((first, rest)) = greetings.split_first() {
            card.first_mes = first.clone();
            card.alternate_greetings = rest.to_vec();
        }
        card.alternate_greetings
            .extend(string_list(data.pointer("/dialogue/group_only_greetings")));
        card.mes_example = string_list(data.pointer("/dialogue/dialogue_examples")).join("\n\n");
        card.system_prompt = s_from(data.pointer("/prompts/system_prompt"));
        card.post_history_instructions = s_from(data.pointer("/prompts/post_history_instructions"));
        card.creator_notes = s_from(data.pointer("/metadata/notes"));
        card.lorebook = parse_lorebook(data.pointer("/world/worldbook_entries"));
    } else {
        card.description = s("description");
        card.personality = s("personality");
        card.scenario = s("scenario");
        card.first_mes = s("first_mes");
        card.alternate_greetings = string_list(data.get("alternate_greetings"));
        card.alternate_greetings
            .extend(string_list(data.get("group_only_greetings")));
        card.mes_example = s("mes_example");
        card.system_prompt = s("system_prompt");
        card.post_history_instructions = s("post_history_instructions");
        card.creator_notes = s("creator_notes");
        card.tags = string_list(data.get("tags"));
        card.lorebook = parse_lorebook(data.pointer("/character_book/entries"));
    }
    Ok(card)
}

fn s_from(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Parse a card from PNG bytes (chara/ccv3 chunks) or raw JSON.
pub fn parse_card(bytes: &[u8]) -> Result<CharacterCard, String> {
    let json_text = if bytes.starts_with(&PNG_SIG) {
        let chunks = png_text_chunks(bytes);
        let pick = |key: &str| {
            chunks
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, t)| t.clone())
        };
        let text = pick("ccv3").or_else(|| pick("chara")).ok_or_else(|| {
            "No character data found in the PNG (chara/ccv3 text chunk missing)".to_string()
        })?;
        let decoded = base64_decode(text.trim())?;
        String::from_utf8(decoded).map_err(|_| "Card data is not valid UTF-8".to_string())?
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| "Card file is not valid UTF-8".to_string())?
    };
    let value: Value =
        serde_json::from_str(&json_text).map_err(|e| format!("Card JSON is invalid: {e}"))?;
    normalize(&value)
}

// ── Storage ───────────────────────────────────────────────────────────────

fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = s.trim_matches('-');
    if s.is_empty() {
        "card".to_string()
    } else {
        s.chars().take(40).collect()
    }
}

fn fnv1a(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn card_id(name: &str, first_mes: &str) -> String {
    format!("{}-{:06x}", slug(name), fnv1a(&format!("{name}\u{1}{first_mes}")) & 0xff_ffff)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub fn roleplay_dir() -> Option<PathBuf> {
    crate::config::data_dir().map(|d| d.join("werk").join("roleplay"))
}

fn cards_dir() -> Result<PathBuf, String> {
    roleplay_dir()
        .map(|d| d.join("cards"))
        .ok_or_else(|| "Cannot find data directory".to_string())
}

/// Per-character memory file; `general` when no card is active.
pub fn character_memory_path(card_id: Option<&str>) -> Option<PathBuf> {
    let id = card_id.filter(|c| valid_id(c)).unwrap_or("general");
    Some(roleplay_dir()?.join("memory").join(format!("{id}.md")))
}

fn card_json_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn card_png_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.png"))
}

fn summary(card: &CharacterCard, has_avatar: bool) -> CardSummary {
    CardSummary {
        id: card.id.clone(),
        name: card.name.clone(),
        spec: card.spec.clone(),
        tags: card.tags.clone(),
        greetings: (1 + card.alternate_greetings.len()) as u32,
        has_avatar,
    }
}

/// Give the card a fresh id, renaming it as a copy (`Name (2)`) when the id
/// already exists.
pub fn make_card_unique(dir: &Path, card: &mut CharacterCard) {
    let base = card.name.trim().to_string();
    let mut n = 1;
    loop {
        let id = card_id(&card.name, &card.first_mes);
        if !card_json_path(dir, &id).exists() {
            card.id = id;
            return;
        }
        n += 1;
        card.name = format!("{base} ({n})");
    }
}

/// `make_card_unique` against the configured cards directory.
pub fn make_card_unique_checked(card: &mut CharacterCard) -> Result<(), String> {
    let dir = cards_dir()?;
    make_card_unique(&dir, card);
    Ok(())
}

pub fn import_card(path: &Path) -> Result<CardSummary, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let is_png = bytes.starts_with(&PNG_SIG);
    let mut card = parse_card(&bytes)?;
    let dir = cards_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Importing a card that already exists lands as a copy.
    make_card_unique(&dir, &mut card);
    let json = serde_json::to_string_pretty(&card).map_err(|e| e.to_string())?;
    std::fs::write(card_json_path(&dir, &card.id), json).map_err(|e| e.to_string())?;
    if is_png {
        std::fs::write(card_png_path(&dir, &card.id), &bytes).map_err(|e| e.to_string())?;
    } else {
        let _ = std::fs::remove_file(card_png_path(&dir, &card.id));
    }
    Ok(summary(&card, is_png))
}

/// Save edits to an existing card. The id is kept: sessions and memory are
/// keyed by it, so renaming the card must not orphan them.
pub fn update_card(mut card: CharacterCard) -> Result<CardSummary, String> {
    if !valid_id(&card.id) {
        return Err("Invalid card id".to_string());
    }
    let dir = cards_dir()?;
    if !card_json_path(&dir, &card.id).exists() {
        return Err("Card not found".to_string());
    }
    card.name = card.name.trim().chars().take(120).collect();
    if card.name.is_empty() {
        card.name = "Unnamed".to_string();
    }
    let cap = |s: String, n: usize| s.chars().take(n).collect::<String>();
    card.description = cap(card.description, 20_000);
    card.personality = cap(card.personality, 20_000);
    card.scenario = cap(card.scenario, 20_000);
    card.first_mes = cap(card.first_mes, 20_000);
    card.mes_example = cap(card.mes_example, 40_000);
    card.post_history_instructions = cap(card.post_history_instructions, 20_000);
    card.alternate_greetings = card
        .alternate_greetings
        .into_iter()
        .take(20)
        .map(|g| cap(g, 20_000))
        .collect();
    card.tags = card
        .tags
        .into_iter()
        .take(20)
        .map(|t| t.trim().chars().take(60).collect::<String>())
        .filter(|t| !t.is_empty())
        .collect();
    let json = serde_json::to_string_pretty(&card).map_err(|e| e.to_string())?;
    std::fs::write(card_json_path(&dir, &card.id), json).map_err(|e| e.to_string())?;
    let has_avatar = card_png_path(&dir, &card.id).exists();
    Ok(summary(&card, has_avatar))
}

pub fn list_cards() -> Result<Vec<CardSummary>, String> {
    let dir = cards_dir()?;
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Ok(out);
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(card) = serde_json::from_str::<CharacterCard>(&text) else {
            continue;
        };
        let has_avatar = card_png_path(&dir, &card.id).exists();
        out.push(summary(&card, has_avatar));
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

pub fn load_card(id: &str) -> Result<CharacterCard, String> {
    if !valid_id(id) {
        return Err("Invalid card id".to_string());
    }
    let dir = cards_dir()?;
    let text = std::fs::read_to_string(card_json_path(&dir, id))
        .map_err(|e| format!("Cannot load card: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Card file is invalid: {e}"))
}

pub fn delete_card(id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("Invalid card id".to_string());
    }
    let dir = cards_dir()?;
    let _ = std::fs::remove_file(card_json_path(&dir, id));
    let _ = std::fs::remove_file(card_png_path(&dir, id));
    Ok(())
}

/// Update a card's own system prompt (used when no custom roleplay prompt is
/// set).
pub fn set_card_prompt(id: &str, prompt: &str) -> Result<(), String> {
    let mut card = load_card(id)?;
    card.system_prompt = prompt.trim().chars().take(20_000).collect();
    let dir = cards_dir()?;
    let json = serde_json::to_string_pretty(&card).map_err(|e| e.to_string())?;
    std::fs::write(card_json_path(&dir, id), json).map_err(|e| e.to_string())
}

pub fn avatar_data_url(id: &str) -> Result<Option<String>, String> {
    if !valid_id(id) {
        return Err("Invalid card id".to_string());
    }
    let dir = cards_dir()?;
    let Ok(bytes) = std::fs::read(card_png_path(&dir, id)) else {
        return Ok(None);
    };
    Ok(Some(format!("data:image/png;base64,{}", base64_encode(&bytes))))
}

/// Raw PNG bytes of a card's avatar, when it has one.
pub fn card_avatar_bytes(id: &str) -> Result<Option<Vec<u8>>, String> {
    if !valid_id(id) {
        return Err("Invalid card id".to_string());
    }
    let dir = cards_dir()?;
    Ok(std::fs::read(card_png_path(&dir, id)).ok())
}

/// Persist a card's avatar PNG (bundle imports).
pub fn write_card_avatar(id: &str, bytes: &[u8]) -> Result<(), String> {
    if !valid_id(id) {
        return Err("Invalid card id".to_string());
    }
    let dir = cards_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(card_png_path(&dir, id), bytes).map_err(|e| e.to_string())
}

pub fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    base64_decode(input)
}

pub fn encode_base64(bytes: &[u8]) -> String {
    base64_encode(bytes)
}

/// Write a card JSON as-is (bundle imports, after `make_card_unique`).
pub fn write_card(card: &CharacterCard) -> Result<(), String> {
    let dir = cards_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(card).map_err(|e| e.to_string())?;
    std::fs::write(card_json_path(&dir, &card.id), json).map_err(|e| e.to_string())
}

/// SillyTavern V2 view of a card for exports; round-trips through `normalize`.
fn export_value(card: &CharacterCard) -> Value {
    let entries: Vec<Value> = card
        .lorebook
        .iter()
        .map(|e| {
            serde_json::json!({
                "keys": e.keys,
                "content": e.content,
                "comment": e.comment,
                "constant": e.constant,
                "enabled": e.enabled,
                "insertion_order": e.order,
            })
        })
        .collect();
    serde_json::json!({
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "data": {
            "name": card.name,
            "description": card.description,
            "personality": card.personality,
            "scenario": card.scenario,
            "first_mes": card.first_mes,
            "mes_example": card.mes_example,
            "creator_notes": card.creator_notes,
            "system_prompt": card.system_prompt,
            "post_history_instructions": card.post_history_instructions,
            "alternate_greetings": card.alternate_greetings,
            "tags": card.tags,
            "character_book": { "entries": entries },
        }
    })
}

/// Rebuild a PNG with the card JSON as a fresh `chara` tEXt chunk before IEND,
/// dropping any existing chara/ccv3 chunks so the new data wins.
fn embed_card_png(png: &[u8], json: &str) -> Result<Vec<u8>, String> {
    if png.len() < 8 || png[..8] != PNG_SIG {
        return Err("Avatar is not a PNG".to_string());
    }
    let mut out = Vec::with_capacity(png.len() + json.len());
    out.extend_from_slice(&PNG_SIG);
    let mut pos = 8usize;
    let mut inserted = false;
    while pos + 12 <= png.len() {
        let len = u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
        if pos + 12 + len > png.len() {
            break;
        }
        let kind = &png[pos + 4..pos + 8];
        let data = &png[pos + 8..pos + 8 + len];
        let is_card_text = kind == b"tEXt" && {
            let nul = data.iter().position(|b| *b == 0).unwrap_or(data.len());
            matches!(&data[..nul], b"chara" | b"ccv3")
        };
        if kind == b"IEND" {
            let text = format!("chara\0{}", base64_encode(json.as_bytes()));
            out.extend_from_slice(&(text.len() as u32).to_be_bytes());
            out.extend_from_slice(b"tEXt");
            out.extend_from_slice(text.as_bytes());
            let mut crc = flate2::Crc::new();
            crc.update(b"tEXt");
            crc.update(text.as_bytes());
            out.extend_from_slice(&crc.sum().to_be_bytes());
            inserted = true;
        }
        if !is_card_text {
            out.extend_from_slice(&png[pos..pos + 12 + len]);
        }
        pos += 12 + len;
    }
    if !inserted {
        return Err("Avatar PNG is missing its IEND chunk".to_string());
    }
    Ok(out)
}

/// Export a card: PNG with an embedded `chara` chunk when the card has an
/// avatar, plain V2 JSON otherwise.
pub fn export_card(id: &str, path: &Path) -> Result<(), String> {
    let card = load_card(id)?;
    let json = serde_json::to_string_pretty(&export_value(&card)).map_err(|e| e.to_string())?;
    let wants_png = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !wants_png {
        return std::fs::write(path, json)
            .map_err(|e| format!("Cannot write {}: {e}", path.display()));
    }
    let png = card_avatar_bytes(id)?
        .ok_or_else(|| "This card has no avatar; export it as JSON instead".to_string())?;
    let bytes = embed_card_png(&png, &json)?;
    std::fs::write(path, bytes).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

// ── Prompt assembly ───────────────────────────────────────────────────────

fn has_placeholder(line: &str) -> bool {
    let mut rest = line;
    while let Some(start) = rest.find("{{") {
        if rest[start + 2..].contains("}}") {
            return true;
        }
        rest = &rest[start + 2..];
    }
    false
}

/// Substitute `{{char}}`/`{{user}}` and drop lines with unresolved
/// placeholders.
pub fn substitute(text: &str, char_name: &str, user_name: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let line = line
            .replace("{{char}}", char_name)
            .replace("{{Char}}", char_name)
            .replace("{{user}}", user_name)
            .replace("{{User}}", user_name);
        if has_placeholder(&line) {
            continue;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

pub fn display_user_name(rp: &RoleplayConfig) -> String {
    let name = rp.user_name.trim();
    if name.is_empty() {
        "User".to_string()
    } else {
        name.to_string()
    }
}

fn user_persona_block(rp: &RoleplayConfig) -> String {
    if rp.user_description.trim().is_empty() {
        return String::new();
    }
    format!(
        "## The user\n{}: {}",
        display_user_name(rp),
        rp.user_description.trim()
    )
}

pub fn character_prompt(card: &CharacterCard, user_name: &str) -> String {
    let sub = |s: &str| substitute(s, &card.name, user_name);
    let mut parts: Vec<String> = Vec::new();
    if !card.description.trim().is_empty() {
        parts.push(format!("## Character\n{}", sub(&card.description)));
    }
    if !card.personality.trim().is_empty() {
        parts.push(format!("## Personality\n{}", sub(&card.personality)));
    }
    if !card.scenario.trim().is_empty() {
        parts.push(format!("## Scenario\n{}", sub(&card.scenario)));
    }
    if !card.mes_example.trim().is_empty() {
        parts.push(format!("## Example dialogue\n{}", sub(&card.mes_example)));
    }
    if !card.post_history_instructions.trim().is_empty() {
        parts.push(format!(
            "## Author's note\n{}",
            sub(&card.post_history_instructions)
        ));
    }
    parts.join("\n\n")
}

/// World-info entries whose keys appear in the recent transcript, plus
/// constant entries, in insertion order.
pub fn lorebook_block(card: &CharacterCard, recent_text: &str, user_name: &str) -> String {
    let haystack = recent_text.to_lowercase();
    let mut hits: Vec<&LorebookEntry> = card
        .lorebook
        .iter()
        .filter(|e| e.enabled)
        .filter(|e| {
            e.constant
                || e.keys
                    .iter()
                    .any(|k| !k.trim().is_empty() && haystack.contains(&k.to_lowercase()))
        })
        .collect();
    hits.sort_by_key(|e| e.order);
    let mut out = String::new();
    for entry in hits {
        let content = substitute(&entry.content, &card.name, user_name);
        if content.is_empty() {
            continue;
        }
        if out.len() + content.len() > LOREBOOK_CAP {
            break;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&content);
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("## World info\n{out}")
    }
}

/// Full roleplay system prompt: the base prompt (custom or built-in), then the
/// card's own prompt, the character blocks, and the user persona.
pub fn system_prompt(config: &AppConfig, card: Option<&CharacterCard>) -> String {
    let user_name = display_user_name(&config.roleplay);
    let char_name = card
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "the character".to_string());
    let base = config
        .roleplay
        .system_prompt
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or(BUILT_IN_ROLEPLAY_PROMPT);
    let mut out = substitute(base, &char_name, &user_name);
    if let Some(card) = card {
        let card_prompt = card.system_prompt.trim();
        if !card_prompt.is_empty() {
            out.push_str("\n\n");
            out.push_str(&substitute(card_prompt, &char_name, &user_name));
        }
        let block = character_prompt(card, &user_name);
        if !block.is_empty() {
            out.push_str("\n\n");
            out.push_str(&block);
        }
    }
    let persona = user_persona_block(&config.roleplay);
    if !persona.is_empty() {
        out.push_str("\n\n");
        out.push_str(&persona);
    }
    out
}

/// The selected greeting with placeholders resolved.
pub fn greeting(card: &CharacterCard, index: usize, user_name: &str) -> Option<String> {
    let text = if index == 0 {
        &card.first_mes
    } else {
        card.alternate_greetings.get(index - 1)?
    };
    let out = substitute(text, &card.name, user_name);
    (!out.trim().is_empty()).then_some(out)
}

/// Injected memory block for the active character.
pub fn memory_block(config: &AppConfig) -> String {
    let Some(path) = character_memory_path(config.roleplay.card_id.as_deref()) else {
        return String::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let capped = if trimmed.chars().count() > harness::memory::MEMORY_BLOCK_CAP {
        let mut short: String = trimmed.chars().take(harness::memory::MEMORY_BLOCK_CAP).collect();
        short.push_str("\n[…]");
        short
    } else {
        trimmed.to_string()
    };
    format!("\n\nMemory:\n{capped}")
}

// ── Commands ──────────────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn roleplay_list_cards() -> Result<Vec<CardSummary>, String> {
    list_cards()
}

#[tauri::command]
#[specta::specta]
pub async fn roleplay_import_card(path: String) -> Result<CardSummary, String> {
    import_card(Path::new(&path))
}

#[tauri::command]
#[specta::specta]
pub async fn roleplay_get_card(id: String) -> Result<CharacterCard, String> {
    load_card(&id)
}

/// Save a card's own system prompt (the fallback when no custom roleplay
/// prompt is set).
#[tauri::command]
#[specta::specta]
pub async fn roleplay_set_card_prompt(id: String, prompt: String) -> Result<(), String> {
    set_card_prompt(&id, &prompt)
}

/// Save edits to an existing card (id kept).
#[tauri::command]
#[specta::specta]
pub async fn roleplay_update_card(card: CharacterCard) -> Result<CardSummary, String> {
    update_card(card)
}

/// Export a card as PNG (embedded V2 data) or JSON.
#[tauri::command]
#[specta::specta]
pub async fn roleplay_export_card(id: String, path: String) -> Result<(), String> {
    export_card(&id, Path::new(&path))
}

#[tauri::command]
#[specta::specta]
pub async fn roleplay_delete_card(
    id: String,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    delete_card(&id)?;
    let mut config = state.config.lock().unwrap();
    if config.roleplay.card_id.as_deref() == Some(id.as_str()) {
        config.roleplay.card_id = None;
        config.roleplay.greeting = 0;
        config.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn roleplay_card_avatar(id: String) -> Result<Option<String>, String> {
    avatar_data_url(&id)
}

#[tauri::command]
#[specta::specta]
pub async fn set_chat_profile(
    profile: ChatProfile,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.chat_profile = profile;
    config.save().map_err(|e| e.to_string())
}

/// Enable/disable the WebUI profile (Settings → Profiles); disabling it also
/// switches away from it.
#[tauri::command]
#[specta::specta]
pub async fn set_webui_enabled(
    enabled: bool,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config.webui_enabled = enabled;
    if !enabled && config.chat_profile == ChatProfile::Webui {
        config.chat_profile = ChatProfile::Agent;
    }
    config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_roleplay_config(
    roleplay: RoleplayConfig,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    let card_id = roleplay.card_id.filter(|c| valid_id(c));
    let card_changed = config.roleplay.card_id != card_id;
    // The avatar is managed by its own command; never cleared by the form.
    let avatar = config.roleplay.user_avatar.clone();
    let reasoning_effort = roleplay
        .reasoning_effort
        .map(|e| e.trim().chars().take(32).collect::<String>())
        .filter(|e| !e.is_empty());
    let system_prompt = roleplay
        .system_prompt
        .map(|p| p.trim().chars().take(20_000).collect::<String>())
        .filter(|p| !p.is_empty());
    config.roleplay = RoleplayConfig {
        card_id,
        greeting: roleplay.greeting,
        user_name: roleplay.user_name.trim().chars().take(80).collect(),
        user_description: roleplay.user_description.chars().take(4_000).collect(),
        temperature: roleplay.temperature,
        top_p: roleplay.top_p,
        repeat_penalty: roleplay.repeat_penalty,
        reasoning_effort,
        system_prompt,
        user_avatar: avatar,
    };
    config.save().map_err(|e| e.to_string())?;
    drop(config);
    if card_changed {
        // Switching characters switches discussions.
        crate::chat::flush_talk(&state);
    }
    Ok(())
}

/// The built-in roleplay prompt, shown when neither the card nor a custom
/// override supplies one.
#[tauri::command]
#[specta::specta]
pub async fn roleplay_system_prompt_default() -> Result<String, String> {
    Ok(BUILT_IN_ROLEPLAY_PROMPT.to_string())
}

/// Import (or clear) the user's avatar for Talk bubbles.
#[tauri::command]
#[specta::specta]
pub async fn roleplay_set_user_avatar(
    path: Option<String>,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    let dir = roleplay_dir()
        .ok_or_else(|| "Cannot find data directory".to_string())?
        .join("avatars");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut config = state.config.lock().unwrap();
    match path {
        Some(p) => {
            let src = Path::new(&p);
            let ext = src
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif"))
                .unwrap_or_else(|| "png".to_string());
            for old in ["png", "jpg", "jpeg", "webp", "gif"] {
                let _ = std::fs::remove_file(dir.join(format!("user.{old}")));
            }
            let target = dir.join(format!("user.{ext}"));
            let bytes =
                std::fs::read(src).map_err(|e| format!("Cannot read avatar: {e}"))?;
            std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
            config.roleplay.user_avatar = Some(target.to_string_lossy().to_string());
        }
        None => {
            for old in ["png", "jpg", "jpeg", "webp", "gif"] {
                let _ = std::fs::remove_file(dir.join(format!("user.{old}")));
            }
            config.roleplay.user_avatar = None;
        }
    }
    config.save().map_err(|e| e.to_string())
}

/// The user's avatar as a data URL, when one is set.
#[tauri::command]
#[specta::specta]
pub async fn roleplay_user_avatar(
    state: State<'_, crate::AppState>,
) -> Result<Option<String>, String> {
    let path = state.config.lock().unwrap().roleplay.user_avatar.clone();
    let Some(path) = path else {
        return Ok(None);
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    let mime = match Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/png",
    };
    Ok(Some(format!("data:{mime};base64,{}", base64_encode(&bytes))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_with_text(keyword: &str, text: &str) -> Vec<u8> {
        let mut out = PNG_SIG.to_vec();
        let push_chunk = |out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&[0, 0, 0, 0]);
        };
        push_chunk(&mut out, b"IHDR", &[0; 13]);
        let mut text_chunk = keyword.as_bytes().to_vec();
        text_chunk.push(0);
        text_chunk.extend_from_slice(text.as_bytes());
        push_chunk(&mut out, b"tEXt", &text_chunk);
        push_chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn parses_v2_png_card() {
        let card = serde_json::json!({
            "spec": "chara_card_v2",
            "spec_version": "2.0",
            "data": {
                "name": "Aria",
                "description": "A {{char}}.",
                "personality": "warm",
                "scenario": "a tavern",
                "first_mes": "Hello {{user}}.",
                "alternate_greetings": ["Alt one"],
                "mes_example": "{{char}}: hi",
                "system_prompt": "",
                "tags": ["fantasy"],
                "character_book": { "entries": [
                    { "keys": ["tavern"], "content": "The tavern is old.", "enabled": true }
                ]}
            }
        });
        let encoded = base64_encode(card.to_string().as_bytes());
        let bytes = png_with_text("chara", &encoded);
        let parsed = parse_card(&bytes).unwrap();
        assert_eq!(parsed.name, "Aria");
        assert_eq!(parsed.alternate_greetings, vec!["Alt one"]);
        assert_eq!(parsed.lorebook.len(), 1);
        assert_eq!(parsed.lorebook[0].keys, vec!["tavern"]);
    }

    #[test]
    fn export_round_trips_through_png_and_json() {
        let card = CharacterCard {
            id: "aria-abc".into(),
            name: "Aria".into(),
            spec: "v2".into(),
            description: "A {{char}}.".into(),
            personality: "warm".into(),
            scenario: "a tavern".into(),
            first_mes: "Hello {{user}}.".into(),
            alternate_greetings: vec!["Alt one".into()],
            mes_example: "{{char}}: hi".into(),
            system_prompt: "Card voice.".into(),
            post_history_instructions: "Stay kind.".into(),
            creator_notes: "notes".into(),
            tags: vec!["fantasy".into()],
            lorebook: vec![LorebookEntry {
                comment: "tavern".into(),
                keys: vec!["tavern".into()],
                content: "The tavern is old.".into(),
                constant: false,
                enabled: true,
                order: 50,
            }],
        };
        let json = serde_json::to_string(&export_value(&card)).unwrap();
        // JSON exports round-trip through the same parser used for imports.
        let parsed = parse_card(json.as_bytes()).unwrap();
        assert_eq!(parsed.name, "Aria");
        assert_eq!(parsed.alternate_greetings, vec!["Alt one"]);
        assert_eq!(parsed.lorebook[0].content, "The tavern is old.");
        assert_eq!(parsed.lorebook[0].order, 50);
        assert_eq!(parsed.system_prompt, "Card voice.");

        // Embedding replaces any existing chara chunk with the current card.
        let stale = png_with_text(
            "chara",
            &base64_encode(b"{\"spec\":\"v1\",\"data\":{\"name\":\"Old\"}}"),
        );
        let embedded = embed_card_png(&stale, &json).unwrap();
        let parsed = parse_card(&embedded).unwrap();
        assert_eq!(parsed.name, "Aria");
        assert_eq!(parsed.lorebook.len(), 1);
    }

    #[test]
    fn ccv3_wins_over_chara() {
        let v2 = serde_json::json!({ "spec": "chara_card_v2", "data": { "name": "Old" } });
        let v3 = serde_json::json!({ "spec": "chara_card_v3", "data": { "name": "New" } });
        let mut bytes = png_with_text("chara", &base64_encode(v2.to_string().as_bytes()));
        let extra = png_with_text("ccv3", &base64_encode(v3.to_string().as_bytes()));
        bytes.extend_from_slice(&extra[8..]);
        let parsed = parse_card(&bytes).unwrap();
        assert_eq!(parsed.name, "New");
    }

    #[test]
    fn parses_aicc_card() {
        let card = serde_json::json!({
            "spec": "aicc_card",
            "spec_version": "1.0",
            "data": {
                "name": "Kisa",
                "general_description": "A college freshman.",
                "appearance": "Red hair.",
                "personality": { "core": "Sarcastic.", "behavior_rules": ["Never passive."] },
                "background_history": "Grew up together.",
                "world_setting_context": "Modern slice of life.",
                "dialogue": {
                    "greetings": ["Hi.", "Yo.", "Hey."],
                    "dialogue_examples": ["{{char}}: hi", "{{user}}: hey"],
                    "group_only_greetings": []
                },
                "prompts": { "system_prompt": "Write Kisa.", "post_history_instructions": "Stay kind." },
                "world": { "worldbook_entries": [
                    { "comment": "Apartment", "keys": ["apartment"], "content": "They share one.", "enabled": true }
                ]},
                "metadata": { "notes": "creator notes" }
            }
        });
        let parsed = parse_card(card.to_string().as_bytes()).unwrap();
        assert_eq!(parsed.name, "Kisa");
        assert!(parsed.description.contains("Red hair."));
        assert!(parsed.personality.contains("Sarcastic."));
        assert!(parsed.scenario.contains("Modern slice of life."));
        assert_eq!(parsed.first_mes, "Hi.");
        assert_eq!(parsed.alternate_greetings, vec!["Yo.", "Hey."]);
        assert_eq!(parsed.system_prompt, "Write Kisa.");
        assert_eq!(parsed.creator_notes, "creator notes");
        assert_eq!(parsed.lorebook.len(), 1);
    }

    #[test]
    fn substitute_drops_unknown_placeholders() {
        let text = "You are {{char}}.\nSecret: {{unknown}}.\nFor {{user}}.";
        let out = substitute(text, "Aria", "Sam");
        assert_eq!(out, "You are Aria.\nFor Sam.");
    }

    #[test]
    fn importing_an_existing_card_lands_as_a_copy() {
        let dir = std::env::temp_dir().join(format!("werk-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut card = CharacterCard {
            id: String::new(),
            name: "Aria".into(),
            spec: "v2".into(),
            description: String::new(),
            personality: String::new(),
            scenario: String::new(),
            first_mes: "Hi.".into(),
            alternate_greetings: Vec::new(),
            mes_example: String::new(),
            system_prompt: String::new(),
            post_history_instructions: String::new(),
            creator_notes: String::new(),
            tags: Vec::new(),
            lorebook: Vec::new(),
        };
        make_card_unique(&dir, &mut card);
        let first = card.id.clone();
        std::fs::write(card_json_path(&dir, &first), "{}").unwrap();
        make_card_unique(&dir, &mut card);
        assert_ne!(card.id, first, "same card must not overwrite");
        assert_eq!(card.name, "Aria (2)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn system_prompt_layers_base_card_and_blocks() {
        let mut config = AppConfig::default();
        config.roleplay.user_name = "Sam".into();
        let blank = CharacterCard {
            id: "x".into(),
            name: "Aria".into(),
            spec: "v2".into(),
            description: String::new(),
            personality: String::new(),
            scenario: String::new(),
            first_mes: String::new(),
            alternate_greetings: Vec::new(),
            mes_example: String::new(),
            system_prompt: String::new(),
            post_history_instructions: String::new(),
            creator_notes: String::new(),
            tags: Vec::new(),
            lorebook: Vec::new(),
        };
        assert!(system_prompt(&config, Some(&blank)).contains("character-driven roleplay"));

        // The card prompt is appended after the base, not used instead of it.
        let mut with_card = blank.clone();
        with_card.system_prompt = "Card voice for {{user}}.".into();
        let out = system_prompt(&config, Some(&with_card));
        assert!(out.contains("Card voice for Sam."), "{out}");
        assert!(
            out.find("character-driven roleplay").unwrap() < out.find("Card voice for Sam.").unwrap()
        );

        // A custom system prompt replaces the built-in but still precedes the card prompt.
        config.roleplay.system_prompt = Some("Custom voice for {{user}}.".into());
        let out = system_prompt(&config, Some(&with_card));
        assert!(out.starts_with("Custom voice for Sam."), "{out}");
        assert!(out.contains("Card voice for Sam."), "{out}");
        assert!(!out.contains("character-driven roleplay"), "{out}");
    }

    #[test]
    fn lorebook_matches_keys() {
        let card = CharacterCard {
            id: "x".into(),
            name: "Aria".into(),
            spec: "v2".into(),
            description: String::new(),
            personality: String::new(),
            scenario: String::new(),
            first_mes: String::new(),
            alternate_greetings: Vec::new(),
            mes_example: String::new(),
            system_prompt: String::new(),
            post_history_instructions: String::new(),
            creator_notes: String::new(),
            tags: Vec::new(),
            lorebook: vec![
                LorebookEntry { comment: "always".into(), keys: vec![], content: "Constant fact.".into(), constant: true, enabled: true, order: 100 },
                LorebookEntry { comment: "hit".into(), keys: vec!["tavern".into()], content: "Tavern fact.".into(), constant: false, enabled: true, order: 50 },
                LorebookEntry { comment: "miss".into(), keys: vec!["dragon".into()], content: "Dragon fact.".into(), constant: false, enabled: true, order: 10 },
            ],
        };
        let block = lorebook_block(&card, "We walk into the Tavern.", "Sam");
        assert!(block.contains("Constant fact."));
        assert!(block.contains("Tavern fact."));
        assert!(!block.contains("Dragon fact."));
        let order = block.find("Tavern fact.").unwrap();
        let constant = block.find("Constant fact.").unwrap();
        assert!(order < constant);
    }

    #[test]
    fn ids_are_slugged_and_stable() {
        let id = card_id("Kisa Yukimura", "Hi.");
        assert!(id.starts_with("kisa-yukimura-"));
        assert_eq!(id, card_id("Kisa Yukimura", "Hi."));
        assert_ne!(id, card_id("Kisa Yukimura", "Yo."));
    }

    #[test]
    fn base64_round_trips() {
        for len in [0usize, 1, 2, 3, 4, 100, 255] {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&bytes)).unwrap(), bytes);
        }
    }

    #[test]
    fn parses_the_local_example_card_when_present() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Kisa Yukimura.aicc.png");
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let card = parse_card(&bytes).unwrap();
        assert_eq!(card.name, "Kisa Yukimura");
        assert_eq!(card.spec, "aicc_card");
        assert_eq!(card.alternate_greetings.len(), 5);
        assert_eq!(card.lorebook.len(), 8);
        assert!(!card.system_prompt.is_empty());
    }
}
