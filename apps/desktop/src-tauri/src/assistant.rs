//! "Tell it what to change": plain-words arrangement edits ("double verse 2", "cut the
//! instrumental and go straight to the bridge"), understood by Claude. It needs the
//! internet and an Anthropic API key, so it's for rehearsal and planning, never for
//! anything on stage: nothing changes until you press Apply on the proposal.
//! Only section names and the arrangement are sent; no audio.

use serde::{Deserialize, Serialize};
use serde_json::json;

const MODEL: &str = "claude-opus-5-5";
const URL: &str = "https://api.anthropic.com/v1/messages";

/// One block of the current arrangement, as the model sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockIn {
    pub section: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub muted: Vec<String>,
}

/// A proposed block: which section, and which current block's edits (muted tracks,
/// trims, note) it keeps (None = a fresh block).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockOut {
    pub section: String,
    pub from: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub blocks: Vec<BlockOut>,
    /// What it did, in a sentence, or why it couldn't.
    pub message: String,
}

const SYSTEM: &str = "You edit the arrangement of a worship song for a music director who runs backing tracks. \
The arrangement is the order the song's sections play in; a section can appear any number of times. \
You get the song's sections (with their length in bars), the current arrangement as numbered blocks, and a request in plain words, \
which may be loose or spoken-style (\"double verse 2\", \"after verse 2 just do chorus 2 twice then the bridge, drop the instrumental\", \"add a tag at the end\"). \
Return the complete new arrangement. Rules: \
use only section names exactly as listed; \
keep every block the request doesn't touch, in the same order; \
for each block give `from`: the number of the current block it comes from (a repeated or moved block keeps the number of the block it copies), or null for a section that wasn't in the arrangement; \
\"double\" or \"repeat\" a section means play it twice in a row where it is; \
musicians' shorthand is normal (V2 = Verse 2, C = Chorus, PC = Pre-Chorus, B = Bridge, Inst = Instrumental, TA = Turnaround, \"the tag\" = Tag); \
when a request names a section without a number and several exist (Chorus 1, Chorus 2), pick the one the context points to, or the one nearest the place being edited. \
In `message`, say in one short plain sentence what you changed. If the request can't be done with these sections or isn't about the arrangement, return the current arrangement unchanged and say why in `message`.";

fn schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "blocks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "section": {"type": "string"},
                        "from": {"anyOf": [{"type": "integer"}, {"type": "null"}]}
                    },
                    "required": ["section", "from"],
                    "additionalProperties": false
                }
            },
            "message": {"type": "string"}
        },
        "required": ["blocks", "message"],
        "additionalProperties": false
    })
}

/// The user message: sections, numbered current blocks, the request.
pub fn prompt(sections: &[(String, f64)], current: &[BlockIn], request: &str) -> String {
    let mut s = String::from("Sections in this song:\n");
    for (n, bars) in sections {
        s.push_str(&format!("- {n} ({} bars)\n", (bars * 10.0).round() / 10.0));
    }
    s.push_str("\nCurrent arrangement:\n");
    for (i, b) in current.iter().enumerate() {
        let mut extra = vec![];
        if !b.note.is_empty() {
            extra.push(format!("note: {}", b.note));
        }
        if !b.muted.is_empty() {
            extra.push(format!("off: {}", b.muted.join(", ")));
        }
        let extra = if extra.is_empty() { String::new() } else { format!("  ({})", extra.join("; ")) };
        s.push_str(&format!("{}. {}{}\n", i + 1, b.section, extra));
    }
    s.push_str(&format!("\nRequest: {}\n", request.trim()));
    s
}

/// The request body for the Messages API.
pub fn body(sections: &[(String, f64)], current: &[BlockIn], request: &str) -> serde_json::Value {
    json!({
        "model": MODEL,
        "max_tokens": 8000,
        "fallbacks": "default",
        "output_config": {"effort": "low", "format": {"type": "json_schema", "schema": schema()}},
        "system": SYSTEM,
        "messages": [{"role": "user", "content": prompt(sections, current, request)}]
    })
}

/// Reads the reply: the JSON proposal, checked against the song. Block numbers in
/// the reply are 1-based; `from` comes back 0-based (None if it doesn't exist).
pub fn parse(reply: &serde_json::Value, sections: &[(String, f64)], current_len: usize) -> Result<Proposal, String> {
    if reply["stop_reason"] == "refusal" {
        return Err("Claude declined that request. Try saying it another way.".into());
    }
    if reply["stop_reason"] == "max_tokens" {
        return Err("The answer was cut off. Try a shorter request.".into());
    }
    let text = reply["content"]
        .as_array()
        .and_then(|c| c.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .ok_or("No answer came back. Try again.")?;
    #[derive(Deserialize)]
    struct Raw {
        blocks: Vec<RawBlock>,
        message: String,
    }
    #[derive(Deserialize)]
    struct RawBlock {
        section: String,
        from: Option<i64>,
    }
    let raw: Raw = serde_json::from_str(text).map_err(|e| format!("The answer wasn't readable ({e}). Try again."))?;
    let mut blocks = vec![];
    for b in raw.blocks {
        let name = sections
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(b.section.trim()))
            .map(|(n, _)| n.clone())
            .ok_or(format!("It suggested \"{}\", which isn't a section of this song. Try again.", b.section))?;
        let from = b.from.filter(|&f| f >= 1 && (f as usize) <= current_len).map(|f| f as usize - 1);
        blocks.push(BlockOut { section: name, from });
    }
    if blocks.is_empty() {
        return Err(if raw.message.is_empty() { "It returned an empty arrangement.".into() } else { raw.message });
    }
    Ok(Proposal { blocks, message: raw.message })
}

/// Asks Claude. Blocking; call off the main thread.
pub fn ask(key: &str, sections: &[(String, f64)], current: &[BlockIn], request: &str) -> Result<Proposal, String> {
    let b = body(sections, current, request);
    let resp = ureq::post(URL)
        .set("x-api-key", key)
        .set("anthropic-version", "2026-10-21")
        .set("anthropic-beta", "server-side-fallback-2026-07-01")
        .set("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(90))
        .send_string(&b.to_string());
    let text = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&detail).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_string)).unwrap_or(detail);
            return Err(match code {
                401 | 403 => "The Anthropic API key was refused. Check it in Setup → Arrangement assistant.".into(),
                429 => "Too many requests just now. Wait a few seconds and try again.".into(),
                529 | 500..=599 => "Claude is busy right now. Try again in a moment.".into(),
                _ => format!("The request failed ({code}): {msg}"),
            });
        }
        Err(e) => return Err(format!("Couldn't reach Claude (no internet?). Typing a plain list still works offline: I, V1, C1 x2, B. ({e})")),
    };
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    parse(&v, sections, current.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs() -> Vec<(String, f64)> {
        ["Intro", "Verse 1", "Chorus 1", "Verse 2", "Chorus 2", "Instrumental 1", "Bridge", "Outro"].iter().map(|n| (n.to_string(), 8.0)).collect()
    }

    #[test]
    fn the_request_carries_the_sections_arrangement_and_words() {
        let cur = vec![BlockIn { section: "Intro".into(), note: String::new(), muted: vec![] }, BlockIn { section: "Verse 2".into(), note: "WL only".into(), muted: vec!["Drums".into()] }];
        let b = body(&secs(), &cur, "double verse 2");
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(b["output_config"]["format"]["type"], "json_schema");
        let p = b["messages"][0]["content"].as_str().unwrap();
        assert!(p.contains("- Instrumental 1 (8 bars)"));
        assert!(p.contains("2. Verse 2  (note: WL only; off: Drums)"));
        assert!(p.ends_with("Request: double verse 2\n"));
    }

    #[test]
    fn reads_and_checks_the_proposal() {
        let reply = json!({"stop_reason": "end_turn", "content": [
            {"type": "thinking", "thinking": ""},
            {"type": "text", "text": "{\"blocks\":[{\"section\":\"intro\",\"from\":1},{\"section\":\"Verse 2\",\"from\":2},{\"section\":\"Verse 2\",\"from\":2},{\"section\":\"Outro\",\"from\":null},{\"section\":\"Bridge\",\"from\":99}],\"message\":\"Doubled Verse 2.\"}"}
        ]});
        let p = parse(&reply, &secs(), 2).unwrap();
        assert_eq!(p.message, "Doubled Verse 2.");
        assert_eq!(p.blocks, vec![
            BlockOut { section: "Intro".into(), from: Some(0) },
            BlockOut { section: "Verse 2".into(), from: Some(1) },
            BlockOut { section: "Verse 2".into(), from: Some(1) },
            BlockOut { section: "Outro".into(), from: None },
            BlockOut { section: "Bridge".into(), from: None },
        ], "names matched to the song's spelling; numbers out of range become fresh blocks");
        let bad = json!({"content": [{"type": "text", "text": "{\"blocks\":[{\"section\":\"Rap\",\"from\":null}],\"message\":\"\"}"}]});
        assert!(parse(&bad, &secs(), 2).unwrap_err().contains("isn't a section"));
        assert!(parse(&json!({"stop_reason": "refusal", "content": []}), &secs(), 2).is_err());
    }
}
