//! Erzeugt `manifest.json` für ein Modpack.
//!
//! Ordnerstruktur:
//!   <packDir>/files/      -> wird immer synchronisiert (mods/, config/, resourcepacks/ …)
//!   <packDir>/defaults/   -> wird nur angelegt, wenn die Datei noch fehlt (z. B. options.txt)
//!
//! cargo run -p launcher-core --bin packtool -- <packDir> --id mein-pack --name "Mein Pack" \
//!     --version 1.0.0 --mc 1.21.4 --loader fabric@0.16.10 [--server play.example.net] \
//!     [--memory 4096] [--description "…"] [--base-url https://cdn.example.net/pack/]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use launcher_core::modpack::{LoaderSpec, Manifest, Memory, PackFile};
use sha1::{Digest, Sha1};

fn walk(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(&path, base, out)?;
        } else if path.is_file() {
            out.push(path.strip_prefix(base)?.to_path_buf());
        }
    }
    Ok(())
}

fn url_path(folder: &str, rel: &str) -> String {
    std::iter::once(folder)
        .chain(rel.split('/'))
        .map(|s| {
            url::form_urlencoded::byte_serialize(s.as_bytes())
                .collect::<String>()
                .replace('+', "%20")
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut positional = vec![];
    let mut opts: HashMap<String, String> = HashMap::new();
    while let Some(a) = args.next() {
        match a.strip_prefix("--") {
            Some(key) => {
                opts.insert(key.to_string(), args.next().unwrap_or_default());
            }
            None => positional.push(a),
        }
    }
    let get = |k: &str| opts.get(k).cloned().unwrap_or_default();
    let Some(pack_dir) = positional.first().map(PathBuf::from) else {
        bail!("Aufruf: packtool <packDir> --id <id> --name <name> --version <x.y.z> --mc <mc> [--loader fabric@<v>] [--server <host>] [--memory <MB>] [--base-url <url>]");
    };
    for k in ["id", "name", "version", "mc"] {
        if get(k).is_empty() {
            bail!("--{k} fehlt");
        }
    }

    let loader = match get("loader").split_once('@') {
        Some((kind, version)) => LoaderSpec {
            kind: kind.into(),
            version: version.into(),
        },
        None => LoaderSpec {
            kind: "vanilla".into(),
            version: String::new(),
        },
    };
    let base_url = match get("base-url") {
        u if u.is_empty() => String::new(),
        u => format!("{}/", u.trim_end_matches('/')),
    };

    let mut files = vec![];
    for (folder, overwrite) in [("files", true), ("defaults", false)] {
        let root = pack_dir.join(folder);
        let mut rels = vec![];
        walk(&root, &root, &mut rels)?;
        rels.sort();
        for rel in rels {
            let data = std::fs::read(root.join(&rel))?;
            let rel = rel.to_string_lossy().replace('\\', "/");
            files.push(PackFile {
                url: base_url.clone() + &url_path(folder, &rel),
                path: rel,
                sha1: Some(hex::encode(Sha1::digest(&data))),
                size: Some(data.len() as u64),
                overwrite: (!overwrite).then_some(false),
            });
        }
    }

    let manifest = Manifest {
        format_version: 1,
        id: get("id"),
        name: get("name"),
        version: get("version"),
        description: get("description"),
        instance_id: get("instance-id"),
        minecraft: get("mc"),
        loader,
        server: get("server"),
        memory: get("memory").parse().ok().map(|max| Memory {
            min: Some(512),
            max,
        }),
        files,
    };
    let out = pack_dir.join("manifest.json");
    std::fs::write(&out, serde_json::to_string_pretty(&manifest)?)?;
    println!(
        "{} geschrieben – {} Dateien, Version {}",
        out.display(),
        manifest.files.len(),
        manifest.version
    );
    Ok(())
}
