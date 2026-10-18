//! Minimaler Qt-INI-Leser/Schreiber für `instance.cfg` und `prismlauncher.cfg`.
//! Unveränderte Werte werden roh zurückgeschrieben, damit Prism-eigene Einträge
//! (z. B. Base64-Spaltenlayouts) exakt erhalten bleiben.

use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Ini {
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    pub fn parse(text: &str) -> Self {
        let mut ini = Ini::default();
        let mut current = "General".to_string();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                current = line[1..line.len() - 1].to_string();
                ini.section_mut(&current);
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                ini.section_mut(&current)
                    .push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        ini
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_string())
    }

    fn section_mut(&mut self, name: &str) -> &mut Vec<(String, String)> {
        let idx = match self.sections.iter().position(|(n, _)| n == name) {
            Some(i) => i,
            None => {
                self.sections.push((name.to_string(), vec![]));
                self.sections.len() - 1
            }
        };
        &mut self.sections[idx].1
    }

    pub fn get(&self, section: &str, key: &str) -> Option<String> {
        self.sections
            .iter()
            .find(|(n, _)| n == section)
            .and_then(|(_, kv)| kv.iter().find(|(k, _)| k == key))
            .map(|(_, v)| decode(v))
    }

    pub fn general(&self, key: &str) -> Option<String> {
        self.get("General", key)
    }

    pub fn has_section(&self, section: &str) -> bool {
        self.sections.iter().any(|(n, _)| n == section)
    }

    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let encoded = encode(value);
        let kv = self.section_mut(section);
        match kv.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = encoded,
            None => kv.push((key.to_string(), encoded)),
        }
    }
}

impl std::fmt::Display for Ini {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, kv) in &self.sections {
            writeln!(f, "[{name}]")?;
            for (k, v) in kv {
                writeln!(f, "{k}={v}")?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

fn decode(raw: &str) -> String {
    let inner = if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('x') => {
                let hex: String = std::iter::from_fn(|| chars.next_if(|c| c.is_ascii_hexdigit()))
                    .take(4)
                    .collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn encode(value: &str) -> String {
    let needs_quotes = value
        .chars()
        .any(|c| matches!(c, ',' | ';' | '"' | '\\' | '='))
        || value.starts_with(' ')
        || value.ends_with(' ');
    if !needs_quotes {
        return value.to_string();
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_raw_and_quotes() {
        let text = "[General]\nname=Test\nnotes=\nmods_Page\\Columns=\"AAAA/w==\"\n\n[UI]\nx=1\n";
        let mut ini = Ini::parse(text);
        assert_eq!(ini.general("name").unwrap(), "Test");
        assert_eq!(
            ini.get("General", "mods_Page\\Columns").unwrap(),
            "AAAA/w=="
        );
        let cmd = r#""C:\Program Files\x.exe" --a "$INST_MC_DIR""#;
        ini.set("General", "PreLaunchCommand", cmd);
        let again = Ini::parse(&ini.to_string());
        assert_eq!(again.general("PreLaunchCommand").unwrap(), cmd);
        assert!(ini.to_string().contains("mods_Page\\Columns=\"AAAA/w==\""));
    }
}
