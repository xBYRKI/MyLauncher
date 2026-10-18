//! Minecraft Server List Ping (Java Edition): MOTD, Spielerzahl, Version, Favicon und Ping.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MotdPart {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Players {
    pub online: i64,
    pub max: i64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub online: bool,
    pub latency: u64,
    pub version: String,
    pub players: Players,
    pub motd: Vec<MotdPart>,
    pub favicon: Option<String>,
    pub error: Option<String>,
}

fn write_varint(buf: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        buf.push(b);
        if v == 0 {
            break;
        }
    }
}

async fn read_varint(stream: &mut TcpStream) -> Result<i32> {
    let mut value: i32 = 0;
    for i in 0..5 {
        let b = stream.read_u8().await?;
        value |= ((b & 0x7f) as i32) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("VarInt zu lang")
}

fn packet(id: i32, body: &[u8]) -> Vec<u8> {
    let mut inner = vec![];
    write_varint(&mut inner, id);
    inner.extend_from_slice(body);
    let mut out = vec![];
    write_varint(&mut out, inner.len() as i32);
    out.extend(inner);
    out
}

async fn resolve(address: &str) -> (String, u16) {
    let address = address.trim();
    if let Some((host, port)) = address.rsplit_once(':') {
        if let Ok(port) = port.parse() {
            return (host.to_string(), port);
        }
    }
    if let Ok(resolver) = hickory_resolver::TokioAsyncResolver::tokio_from_system_conf() {
        if let Ok(lookup) = resolver
            .srv_lookup(format!("_minecraft._tcp.{address}."))
            .await
        {
            if let Some(srv) = lookup.iter().next() {
                return (
                    srv.target().to_utf8().trim_end_matches('.').to_string(),
                    srv.port(),
                );
            }
        }
    }
    (address.to_string(), 25565)
}

// ---------- MOTD ----------

fn legacy_color(code: char) -> Option<&'static str> {
    Some(match code {
        '0' => "black",
        '1' => "dark_blue",
        '2' => "dark_green",
        '3' => "dark_aqua",
        '4' => "dark_red",
        '5' => "dark_purple",
        '6' => "gold",
        '7' => "gray",
        '8' => "dark_gray",
        '9' => "blue",
        'a' => "green",
        'b' => "aqua",
        'c' => "red",
        'd' => "light_purple",
        'e' => "yellow",
        'f' => "white",
        _ => return None,
    })
}

/// Text mit §-Codes in Teile zerlegen.
fn split_legacy(text: &str, base: &MotdPart, out: &mut Vec<MotdPart>) {
    let mut style = MotdPart {
        text: String::new(),
        ..base.clone()
    };
    for (i, part) in text.split('§').enumerate() {
        let rest = if i == 0 {
            part
        } else {
            let mut chars = part.chars();
            match chars.next().map(|c| c.to_ascii_lowercase()) {
                Some(c) if legacy_color(c).is_some() => {
                    style = MotdPart {
                        color: legacy_color(c).map(String::from),
                        ..Default::default()
                    }
                }
                Some('l') => style.bold = true,
                Some('o') => style.italic = true,
                Some('r') => style = MotdPart::default(),
                _ => {}
            }
            chars.as_str()
        };
        if !rest.is_empty() {
            out.push(MotdPart {
                text: rest.to_string(),
                ..style.clone()
            });
        }
    }
}

fn flatten_chat(node: &Value, inherited: &MotdPart, out: &mut Vec<MotdPart>) {
    match node {
        Value::String(s) => split_legacy(s, inherited, out),
        Value::Array(items) => items.iter().for_each(|n| flatten_chat(n, inherited, out)),
        Value::Object(obj) => {
            let mut style = inherited.clone();
            if let Some(c) = obj.get("color").and_then(Value::as_str) {
                style.color = Some(c.to_string());
            }
            if let Some(b) = obj.get("bold").and_then(Value::as_bool) {
                style.bold = b;
            }
            if let Some(b) = obj.get("italic").and_then(Value::as_bool) {
                style.italic = b;
            }
            if let Some(t) = obj.get("text").and_then(Value::as_str) {
                split_legacy(t, &style, out);
            }
            if let Some(extra) = obj.get("extra") {
                flatten_chat(extra, &style, out);
            }
        }
        _ => {}
    }
}

// ---------- Ping ----------

async fn ping(address: &str) -> Result<ServerStatus> {
    let (host, port) = resolve(address).await;
    let mut stream = TcpStream::connect((host.as_str(), port)).await?;

    let mut handshake = vec![];
    write_varint(&mut handshake, -1); // Protokoll egal für Status
    write_varint(&mut handshake, host.len() as i32);
    handshake.extend_from_slice(host.as_bytes());
    handshake.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut handshake, 1);
    stream.write_all(&packet(0x00, &handshake)).await?;

    let sent = Instant::now();
    stream.write_all(&packet(0x00, &[])).await?;

    let _len = read_varint(&mut stream).await?;
    let latency = sent.elapsed().as_millis() as u64;
    let _id = read_varint(&mut stream).await?;
    let str_len = read_varint(&mut stream).await?;
    if !(0..=1 << 21).contains(&str_len) {
        bail!("Ungültige Antwort");
    }
    let mut buf = vec![0u8; str_len as usize];
    stream.read_exact(&mut buf).await?;
    let json: Value = serde_json::from_slice(&buf)?;

    let mut motd = vec![];
    flatten_chat(&json["description"], &MotdPart::default(), &mut motd);
    Ok(ServerStatus {
        online: true,
        latency,
        version: json["version"]["name"].as_str().unwrap_or_default().into(),
        players: Players {
            online: json["players"]["online"].as_i64().unwrap_or(0),
            max: json["players"]["max"].as_i64().unwrap_or(0),
        },
        motd,
        favicon: json["favicon"]
            .as_str()
            .filter(|f| f.starts_with("data:image/"))
            .map(String::from),
        error: None,
    })
}

pub async fn status(address: &str) -> ServerStatus {
    let result = timeout(Duration::from_secs(6), ping(address))
        .await
        .map_err(|_| anyhow!("Zeitüberschreitung"))
        .and_then(|r| r);
    result.unwrap_or_else(|e| ServerStatus {
        error: Some(e.to_string()),
        ..Default::default()
    })
}
