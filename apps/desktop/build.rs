//! Collects every library compiled into Timewarp, with its licence and the
//! licence texts it ships, for About → Acknowledgements. Reads Cargo's own
//! dependency data, so the list always matches what's in the app.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    println!("cargo:rerun-if-changed={}", root.join("Cargo.lock").display());
    println!("cargo:rerun-if-changed=build.rs");
    commit(&root);
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("licenses.json");
    let data = collect(&root).unwrap_or_else(|e| {
        println!("cargo:warning=couldn't list licences: {e}");
        json!({ "packages": [], "texts": [] })
    });
    std::fs::write(out, serde_json::to_vec(&data).unwrap()).unwrap();
}

/// The commit this was built from, for About and bug reports. Empty when
/// there's no git (e.g. a source tarball).
fn commit(root: &Path) {
    let git = root.join(".git");
    for p in ["HEAD", "refs/heads", "packed-refs"] {
        println!("cargo:rerun-if-changed={}", git.join(p).display());
    }
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .or_else(|| std::env::var("GITHUB_SHA").ok().map(|s| s.chars().take(8).collect()))
        .unwrap_or_default();
    println!("cargo:rustc-env=TIMEWARP_COMMIT={sha}");
}

fn collect(root: &Path) -> Result<Value, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let target = std::env::var("TARGET").map_err(|e| e.to_string())?;
    let out = std::process::Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--offline", "--filter-platform", &target])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let m: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let packages: HashMap<&str, &Value> =
        m["packages"].as_array().unwrap().iter().map(|p| (p["id"].as_str().unwrap(), p)).collect();
    let nodes: HashMap<&str, &Value> =
        m["resolve"]["nodes"].as_array().unwrap().iter().map(|n| (n["id"].as_str().unwrap(), n)).collect();
    let workspace: HashSet<&str> = m["workspace_members"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    let me = packages
        .values()
        .find(|p| p["name"] == "timewarp-desktop")
        .and_then(|p| p["id"].as_str())
        .ok_or("timewarp-desktop not found")?;

    // Libraries our own crates use directly (not what those pull in):
    // walk the app and the workspace crates it uses, stop at the first
    // outside library.
    let mut seen = HashSet::new();
    let mut ours = HashSet::from([me]);
    let mut stack = vec![me];
    while let Some(id) = stack.pop() {
        for d in nodes[id]["deps"].as_array().into_iter().flatten() {
            let normal = d["dep_kinds"].as_array().into_iter().flatten().any(|k| k["kind"].is_null());
            let pkg = d["pkg"].as_str().unwrap();
            if !normal {
                continue;
            }
            if workspace.contains(pkg) {
                if ours.insert(pkg) {
                    stack.push(pkg);
                }
            } else {
                seen.insert(pkg);
            }
        }
    }

    let mut texts: Vec<String> = vec![];
    let mut text_ids: HashMap<String, usize> = HashMap::new();
    let mut list: BTreeMap<(String, String), Value> = BTreeMap::new();
    let mut pending_apache: Vec<String> = vec![];
    for id in seen.into_iter().filter(|id| !workspace.contains(id)) {
        let p = packages[id];
        let name = p["name"].as_str().unwrap_or_default().to_string();
        let version = p["version"].as_str().unwrap_or_default().to_string();
        let dir = Path::new(p["manifest_path"].as_str().unwrap_or_default()).parent().unwrap().to_path_buf();
        let mut files = licence_files(&dir);
        if let Some(f) = p["license_file"].as_str() {
            files.push(dir.join(f));
        }
        files.sort();
        files.dedup();
        let mut ids = vec![];
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            let text = text.replace("\r\n", "\n").trim().to_string();
            if text.is_empty() {
                continue;
            }
            let next = texts.len();
            let i = *text_ids.entry(text.clone()).or_insert_with(|| {
                texts.push(text);
                next
            });
            if !ids.contains(&i) {
                ids.push(i);
            }
        }
        // No file shipped: fall back to the standard text of the licence
        // it declares (MIT with its authors, or Apache-2.0).
        let declared = p["license"].as_str().unwrap_or_default();
        let mut standard = false;
        if ids.is_empty() && declared.contains("MIT") {
            let authors: Vec<String> = p["authors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str())
                .map(|a| a.split(" <").next().unwrap_or(a).to_string())
                .collect();
            let holder = if authors.is_empty() { format!("The {name} authors") } else { authors.join(", ") };
            let text = MIT.replace("{holder}", &holder);
            let next = texts.len();
            let i = *text_ids.entry(text.clone()).or_insert_with(|| {
                texts.push(text);
                next
            });
            ids.push(i);
            standard = true;
        } else if ids.is_empty() && declared.contains("Apache-2.0") {
            pending_apache.push(name.to_lowercase() + "@" + &version);
            standard = true;
        }
        let licence = p["license"]
            .as_str()
            .map(|l| l.replace(" / ", " OR ").replace('/', " OR "))
            .unwrap_or_else(|| "See licence text".into());
        let link = p["repository"].as_str().or(p["homepage"].as_str()).unwrap_or_default();
        list.insert(
            (name.to_lowercase(), version.clone()),
            json!({
                "name": name,
                "version": version,
                "license": licence,
                "url": link,
                "description": p["description"].as_str().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "),
                "texts": ids,
                "standard": standard,
            }),
        );
    }
    // Apache-2.0 is the same text everywhere: reuse a shipped copy.
    if !pending_apache.is_empty() {
        let apache = texts
            .iter()
            .position(|t| t.starts_with("Apache License") && t.contains("Version 2.0") && t.len() > 9000);
        if let Some(i) = apache {
            for ((n, v), p) in list.iter_mut() {
                if pending_apache.contains(&format!("{n}@{v}")) {
                    p["texts"] = json!([i]);
                }
            }
        }
    }
    Ok(json!({ "packages": list.into_values().collect::<Vec<_>>(), "texts": texts }))
}

fn licence_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy().to_uppercase();
            ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"].iter().any(|k| n.starts_with(k))
        })
        .collect()
}

const MIT: &str = "MIT License

Copyright (c) {holder}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.";
