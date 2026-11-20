//! Device configuration files in Z-Wave JS's published format.
//!
//! The Z-Wave JS project keeps a library of device files (`@zwave-js/config`,
//! MIT): each describes devices by manufacturer, product type and product id,
//! with their configuration parameters, associations and compatibility flags.
//! This module reads those files as published, without converting them:
//!
//! - JSON5 text (comments, trailing commas);
//! - `$import` of templates: `~/path.json#selector` from the library root,
//!   `path.json#selector` relative to the importing file, `#selector` within
//!   it; a selector walks properties and picks parameters by their `#`;
//!   the imported object is merged where the `$import` key stands, later keys
//!   overriding it;
//! - `$if` conditions over `firmwareVersion` and `sdkVersion` (compared as
//!   versions) and `manufacturerId`, `productType` and `productId`;
//!   conditional values take the first entry that applies;
//! - parameters, including partial parameters (`"41[0xf00]"`), associations,
//!   compatibility flags, endpoints and metadata.
//!
//! A [`Library`] reads from a [`Source`]: a directory (the package's
//! `config/devices`) or an in-memory map of paths to text, such as a single
//! device's file and the templates it needs.

use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use zwave_cc::{configuration, Command};

/// Largest config file accepted.
pub const MAX_FILE: usize = 4 * 1024 * 1024;
/// Deepest chain of `$import`s accepted.
pub const MAX_IMPORT_DEPTH: usize = 32;

/// A problem with a config file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{file}: {message}")]
pub struct ConfigError {
    /// The file (relative to the library root).
    pub file: String,
    /// What is wrong.
    pub message: String,
}

fn err(file: &str, message: impl Into<String>) -> ConfigError {
    ConfigError {
        file: file.to_string(),
        message: message.into(),
    }
}

/// Where config files come from. Paths are relative to the library root
/// (the package's `config/devices`), with `/` separators.
pub trait Source: Send + Sync {
    /// The text of a file, if it exists.
    fn read(&self, path: &str) -> Option<String>;
    /// Every `.json` file's path.
    fn paths(&self) -> Vec<String>;
}

/// Files in a directory.
#[derive(Debug, Clone)]
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    /// The package's `config/devices` directory (or a copy of it).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        DirSource { root: root.into() }
    }
}

impl Source for DirSource {
    fn read(&self, path: &str) -> Option<String> {
        let p = self.root.join(path);
        let meta = std::fs::metadata(&p).ok()?;
        if !meta.is_file() || meta.len() as usize > MAX_FILE {
            return None;
        }
        std::fs::read_to_string(p).ok()
    }

    fn paths(&self) -> Vec<String> {
        fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let name = e.file_name().to_string_lossy().to_string();
                let rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path(), &rel, out),
                    Ok(t) if t.is_file() && name.ends_with(".json") => out.push(rel),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, "", &mut out);
        out
    }
}

/// Files held in memory, keyed by path.
#[derive(Debug, Clone, Default)]
pub struct MapSource {
    files: BTreeMap<String, String>,
}

impl MapSource {
    /// A source over these files.
    pub fn new(files: BTreeMap<String, String>) -> Self {
        MapSource { files }
    }
}

impl Source for MapSource {
    fn read(&self, path: &str) -> Option<String> {
        self.files
            .get(path)
            .filter(|t| t.len() <= MAX_FILE)
            .cloned()
    }

    fn paths(&self) -> Vec<String> {
        self.files
            .keys()
            .filter(|k| k.ends_with(".json"))
            .cloned()
            .collect()
    }
}

/// Joins and normalizes a path inside the library; `None` if it escapes the root.
fn normalize(base_dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<String> = base_dir
        .split('/')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().to_string()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Reads device files from a [`Source`], caching parsed files.
pub struct Library {
    src: Box<dyn Source>,
    parsed: Mutex<HashMap<String, Arc<Value>>>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Library(..)")
    }
}

/// The ids a device file is chosen and evaluated by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceId {
    /// Manufacturer id.
    pub manufacturer_id: u16,
    /// Product type.
    pub product_type: u16,
    /// Product id.
    pub product_id: u16,
    /// Firmware version, `major.minor[.patch]`.
    pub firmware_version: String,
    /// Z-Wave SDK version, if known.
    pub sdk_version: Option<String>,
}

/// One device file's identity, without resolving its imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// Path relative to the root.
    pub path: String,
    /// Manufacturer id.
    pub manufacturer_id: u16,
    /// Product type and product id pairs it covers.
    pub devices: Vec<(u16, u16)>,
    /// Firmware range it covers.
    pub firmware: (String, String),
    /// Marked `preferred` over other files for the same ids.
    pub preferred: bool,
}

fn parse_hex(v: &Value) -> Option<u16> {
    let s = v.as_str()?;
    u16::from_str_radix(s.strip_prefix("0x").or(s.strip_prefix("0X"))?, 16).ok()
}

impl Library {
    /// A library reading from `src`.
    pub fn new(src: impl Source + 'static) -> Self {
        Library {
            src: Box::new(src),
            parsed: Mutex::new(HashMap::new()),
        }
    }

    fn parse(&self, path: &str) -> Result<Arc<Value>, ConfigError> {
        if let Some(v) = self.parsed.lock().expect("lock").get(path) {
            return Ok(v.clone());
        }
        let text = self.src.read(path).ok_or_else(|| err(path, "not found"))?;
        let v: Value = json5::from_str(&text).map_err(|e| err(path, e.to_string()))?;
        let v = Arc::new(v);
        // Only templates are worth keeping; device files are read once.
        if path.contains("templates/") {
            self.parsed
                .lock()
                .expect("lock")
                .insert(path.to_string(), v.clone());
        }
        Ok(v)
    }

    /// A file with every `$import` resolved (conditions not yet evaluated).
    pub fn resolve(&self, path: &str) -> Result<Value, ConfigError> {
        self.resolve_file(path, None, &mut Vec::new())
    }

    fn resolve_file(
        &self,
        path: &str,
        selector: Option<&str>,
        stack: &mut Vec<String>,
    ) -> Result<Value, ConfigError> {
        let key = format!("{path}#{}", selector.unwrap_or(""));
        if stack.contains(&key) {
            return Err(err(
                path,
                format!("circular $import: {} -> {key}", stack.join(" -> ")),
            ));
        }
        if stack.len() >= MAX_IMPORT_DEPTH {
            return Err(err(path, "$import chain too deep"));
        }
        let json = self.parse(path)?;
        let target = match selector {
            Some(s) => select(&json, s)
                .ok_or_else(|| err(path, format!("import target #{s} is not an object")))?,
            None => (*json).clone(),
        };
        stack.push(key);
        let r = self.resolve_imports(target, path, stack);
        stack.pop();
        r
    }

    fn resolve_imports(
        &self,
        v: Value,
        file: &str,
        stack: &mut Vec<String>,
    ) -> Result<Value, ConfigError> {
        match v {
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, val) in m {
                    if k == "$import" {
                        let spec = val
                            .as_str()
                            .ok_or_else(|| err(file, "$import must be a string"))?;
                        let (fname, selector) = match spec.split_once('#') {
                            Some((f, s)) => (f, Some(s)),
                            None => (spec, None),
                        };
                        let target = if fname.is_empty() {
                            file.to_string()
                        } else if let Some(rooted) = fname.strip_prefix("~/") {
                            normalize("", rooted)
                                .ok_or_else(|| err(file, format!("bad import path {spec}")))?
                        } else {
                            normalize(dir_of(file), fname).ok_or_else(|| {
                                err(file, format!("import {spec} leaves the library"))
                            })?
                        };
                        if !target.ends_with(".json") {
                            return Err(err(file, format!("bad import {spec}")));
                        }
                        let imported =
                            self.resolve_file(&target, selector.filter(|s| !s.is_empty()), stack)?;
                        if let Value::Object(im) = imported {
                            for (ik, iv) in im {
                                out.insert(ik, iv);
                            }
                        }
                    } else {
                        out.insert(k, self.resolve_imports(val, file, stack)?);
                    }
                }
                Ok(Value::Object(out))
            }
            Value::Array(a) => Ok(Value::Array(
                a.into_iter()
                    .map(|x| self.resolve_imports(x, file, stack))
                    .collect::<Result<_, _>>()?,
            )),
            other => Ok(other),
        }
    }

    /// Every device file's identity (template files excluded).
    pub fn index(&self) -> Vec<Result<IndexEntry, ConfigError>> {
        self.src
            .paths()
            .into_iter()
            .filter(|p| !p.split('/').any(|c| c == "templates") && p.contains('/'))
            .map(|p| {
                let v = self.parse(&p)?;
                let mfr = v
                    .get("manufacturerId")
                    .and_then(parse_hex)
                    .ok_or_else(|| err(&p, "manufacturerId missing"))?;
                let devices = v
                    .get("devices")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|d| {
                                Some((
                                    d.get("productType").and_then(parse_hex)?,
                                    d.get("productId").and_then(parse_hex)?,
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let fw = v.get("firmwareVersion");
                let s = |k: &str| {
                    fw.and_then(|f| f.get(k))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                Ok(IndexEntry {
                    path: p.clone(),
                    manufacturer_id: mfr,
                    devices,
                    firmware: (s("min"), s("max")),
                    preferred: v.get("preferred").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect()
    }

    /// Finds the file for a device and evaluates it for its firmware.
    pub fn lookup(&self, id: &DeviceId) -> Result<Option<DeviceConfig>, ConfigError> {
        let mut candidates: Vec<IndexEntry> = self
            .index()
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| {
                e.manufacturer_id == id.manufacturer_id
                    && e.devices.contains(&(id.product_type, id.product_id))
                    && version_cmp(&id.firmware_version, &e.firmware.0) != std::cmp::Ordering::Less
                    && version_cmp(&id.firmware_version, &e.firmware.1)
                        != std::cmp::Ordering::Greater
            })
            .collect();
        candidates.sort_by_key(|e| !e.preferred);
        match candidates.first() {
            Some(e) => self.load(&e.path, id).map(Some),
            None => Ok(None),
        }
    }

    /// Loads one file and evaluates it for a device.
    pub fn load(&self, path: &str, id: &DeviceId) -> Result<DeviceConfig, ConfigError> {
        let v = self.resolve(path)?;
        DeviceConfig::from_value(path, &v, id)
    }
}

/// Walks a selector: properties, or in arrays the entry whose `#` matches (without its `#`).
fn select(v: &Value, selector: &str) -> Option<Value> {
    let mut cur = v.clone();
    for part in selector.split('/').filter(|s| !s.is_empty()) {
        cur = match cur {
            Value::Array(a) => {
                let mut item = a
                    .into_iter()
                    .find(|i| i.get("#").and_then(Value::as_str) == Some(part))?;
                item.as_object_mut()?.shift_remove("#");
                item
            }
            Value::Object(mut m) => m.shift_remove(part)?,
            _ => return None,
        };
    }
    cur.is_object().then_some(cur)
}

// ------------------------------------------------------------------ conditions

/// Compares versions component by component (`1.10` > `1.9`), missing components as 0.
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (mut x, mut y) = (parse(a), parse(b));
    let n = x.len().max(y.len()).max(3);
    x.resize(n, 0);
    y.resize(n, 0);
    x.cmp(&y)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Lit(String),
    Op(&'static str),
    LParen,
    RParen,
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'(' {
            out.push(Tok::LParen);
            i += 1;
        } else if c == b')' {
            out.push(Tok::RParen);
            i += 1;
        } else if c.is_ascii_alphabetic() {
            let st = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(s[st..i].to_string()));
        } else if c.is_ascii_digit() {
            let st = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            out.push(Tok::Lit(s[st..i].to_string()));
        } else {
            let rest = &s[i..];
            let op = ["===", "!==", ">=", "<=", "&&", "||", ">", "<", "!"]
                .into_iter()
                .find(|o| rest.starts_with(o))?;
            out.push(Tok::Op(op));
            i += op.len();
        }
    }
    Some(out)
}

struct Parser<'a> {
    t: &'a [Tok],
    i: usize,
    id: &'a DeviceId,
}

impl Parser<'_> {
    fn or(&mut self) -> Option<bool> {
        let mut v = self.and()?;
        while self.t.get(self.i) == Some(&Tok::Op("||")) {
            self.i += 1;
            let r = self.and()?;
            v = v || r;
        }
        Some(v)
    }
    fn and(&mut self) -> Option<bool> {
        let mut v = self.unary()?;
        while self.t.get(self.i) == Some(&Tok::Op("&&")) {
            self.i += 1;
            let r = self.unary()?;
            v = v && r;
        }
        Some(v)
    }
    fn unary(&mut self) -> Option<bool> {
        match self.t.get(self.i)? {
            Tok::Op("!") => {
                self.i += 1;
                Some(!self.unary()?)
            }
            Tok::LParen => {
                self.i += 1;
                let v = self.or()?;
                (self.t.get(self.i)? == &Tok::RParen).then_some(())?;
                self.i += 1;
                Some(v)
            }
            _ => self.cmp(),
        }
    }
    fn cmp(&mut self) -> Option<bool> {
        let Tok::Ident(name) = self.t.get(self.i)?.clone() else {
            return None;
        };
        let Tok::Op(op) = *self.t.get(self.i + 1)? else {
            return None;
        };
        let Tok::Lit(lit) = self.t.get(self.i + 2)?.clone() else {
            return None;
        };
        self.i += 3;
        use std::cmp::Ordering::*;
        let ord = match name.as_str() {
            "firmwareVersion" => Some(version_cmp(&self.id.firmware_version, &lit)),
            // Unknown SDK version: no comparison holds.
            "sdkVersion" => self.id.sdk_version.as_deref().map(|v| version_cmp(v, &lit)),
            "manufacturerId" | "productType" | "productId" => {
                let have = match name.as_str() {
                    "manufacturerId" => self.id.manufacturer_id,
                    "productType" => self.id.product_type,
                    _ => self.id.product_id,
                };
                let want = match lit.strip_prefix("0x").or(lit.strip_prefix("0X")) {
                    Some(h) => u64::from_str_radix(h, 16).ok()?,
                    None => lit.parse().ok()?,
                };
                Some(u64::from(have).cmp(&want))
            }
            _ => return None,
        };
        let Some(ord) = ord else { return Some(false) };
        Some(match op {
            "===" => ord == Equal,
            "!==" => ord != Equal,
            ">=" => ord != Less,
            "<=" => ord != Greater,
            ">" => ord == Greater,
            "<" => ord == Less,
            _ => return None,
        })
    }
}

/// Evaluates a `$if` expression for a device; `Err` for an expression this parser does not understand.
pub fn evaluate(expr: &str, id: &DeviceId) -> Result<bool, String> {
    let toks = lex(expr).ok_or_else(|| format!("cannot read condition {expr:?}"))?;
    let mut p = Parser { t: &toks, i: 0, id };
    let v = p
        .or()
        .ok_or_else(|| format!("cannot read condition {expr:?}"))?;
    if p.i != toks.len() {
        return Err(format!("cannot read condition {expr:?}"));
    }
    Ok(v)
}

/// Applies `$if` throughout a value: objects whose condition fails disappear
/// (from arrays and maps), `$if` keys are removed.
fn deep(v: &Value, id: &DeviceId, file: &str) -> Result<Option<Value>, ConfigError> {
    Ok(match v {
        Value::Object(m) => {
            if let Some(c) = m.get("$if") {
                let c = c
                    .as_str()
                    .ok_or_else(|| err(file, "$if must be a string"))?;
                if !evaluate(c, id).map_err(|e| err(file, e))? {
                    return Ok(None);
                }
            }
            let mut out = Map::new();
            for (k, x) in m {
                if k == "$if" {
                    continue;
                }
                if let Some(x) = deep(x, id, file)? {
                    out.insert(k.clone(), x);
                }
            }
            Some(Value::Object(out))
        }
        Value::Array(a) => {
            let mut out = Vec::new();
            for x in a {
                if let Some(x) = deep(x, id, file)? {
                    out.push(x);
                }
            }
            Some(Value::Array(out))
        }
        other => Some(other.clone()),
    })
}

/// A conditional primitive: the value itself, or the first applicable entry of
/// `[{"$if": .., "value": ..}, .., fallback]`.
fn first_value(v: Option<&Value>, id: &DeviceId, file: &str) -> Result<Option<Value>, ConfigError> {
    match v {
        Some(Value::Array(a)) => {
            for item in a {
                match item {
                    Value::Object(m) => {
                        let ok = match m.get("$if").and_then(Value::as_str) {
                            Some(c) => evaluate(c, id).map_err(|e| err(file, e))?,
                            None => true,
                        };
                        if ok {
                            return Ok(m.get("value").cloned());
                        }
                    }
                    other => return Ok(Some(other.clone())),
                }
            }
            Ok(None)
        }
        Some(other) => Ok(Some(other.clone())),
        None => Ok(None),
    }
}

/// The first applicable variant of a value that may be an array of conditional objects.
fn first_object(
    v: Option<&Value>,
    id: &DeviceId,
    file: &str,
) -> Result<Option<Value>, ConfigError> {
    match v {
        Some(Value::Array(a)) => {
            for item in a {
                if let Some(x) = deep(item, id, file)? {
                    return Ok(Some(x));
                }
            }
            Ok(None)
        }
        Some(other) => deep(other, id, file),
        None => Ok(None),
    }
}

// --------------------------------------------------------------- evaluated

/// A configuration parameter as the file describes it for this device.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamConfig {
    /// Parameter number.
    pub parameter: u16,
    /// Bit mask of a partial parameter.
    pub mask: Option<u32>,
    /// The key as written in the file: `"41"` or `"41[0xf00]"`.
    pub key: String,
    /// Label.
    pub label: String,
    /// Description.
    pub description: Option<String>,
    /// Size in bytes: 1, 2 or 4.
    pub size: u8,
    /// Minimum value.
    pub min: i64,
    /// Maximum value.
    pub max: i64,
    /// Default value.
    pub default: i64,
    /// The value is unsigned.
    pub unsigned: bool,
    /// Cannot be written.
    pub read_only: bool,
    /// Cannot be read.
    pub write_only: bool,
    /// Values other than the options may be entered.
    pub allow_manual_entry: bool,
    /// Named values.
    pub options: Vec<(i64, String)>,
    /// Unit.
    pub unit: Option<String>,
}

/// An association group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociationConfig {
    /// Label.
    pub label: String,
    /// Most nodes the group holds.
    pub max_nodes: u8,
    /// The lifeline group (reports to the controller).
    pub is_lifeline: bool,
    /// Multi Channel association required, if stated.
    pub multi_channel: Option<bool>,
}

/// A device file evaluated for one device.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceConfig {
    /// The file it came from.
    pub file: String,
    /// Manufacturer name.
    pub manufacturer: String,
    /// Manufacturer id.
    pub manufacturer_id: u16,
    /// Product label.
    pub label: String,
    /// Product description.
    pub description: String,
    /// Product type and id pairs.
    pub devices: Vec<(u16, u16)>,
    /// Firmware range.
    pub firmware: (String, String),
    /// Association groups.
    pub associations: BTreeMap<u8, AssociationConfig>,
    /// Parameters, in file order, keyed as in the file.
    pub params: Vec<ParamConfig>,
    /// Compatibility flags (as in the file, conditions applied).
    pub compat: Option<Value>,
    /// Endpoint definitions (conditions applied).
    pub endpoints: Option<Value>,
    /// Inclusion, exclusion and reset instructions (conditions applied).
    pub metadata: Option<Value>,
}

fn int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => match s.strip_prefix("0x") {
            Some(h) => i64::from_str_radix(h, 16).ok(),
            None => s.parse().ok(),
        },
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn string(v: Option<Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s),
        other => Some(other.to_string()),
    }
}

/// Splits `"41[0xf00]"` into the parameter and the mask.
pub fn parse_param_key(key: &str) -> Option<(u16, Option<u32>)> {
    match key.split_once('[') {
        Some((p, m)) => {
            let m = m.strip_suffix(']')?;
            let m = u32::from_str_radix(m.strip_prefix("0x").or(m.strip_prefix("0X"))?, 16).ok()?;
            Some((p.trim().parse().ok()?, Some(m).filter(|&m| m != 0)))
        }
        None => Some((key.trim().parse().ok()?, None)),
    }
}

impl DeviceConfig {
    fn from_value(file: &str, v: &Value, id: &DeviceId) -> Result<Self, ConfigError> {
        let s = |k: &str| -> Result<Option<String>, ConfigError> {
            Ok(string(first_value(v.get(k), id, file)?))
        };
        let manufacturer_id = v
            .get("manufacturerId")
            .and_then(parse_hex)
            .ok_or_else(|| err(file, "manufacturerId missing"))?;
        let devices = v
            .get("devices")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|d| {
                        Some((
                            d.get("productType").and_then(parse_hex)?,
                            d.get("productId").and_then(parse_hex)?,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let fw = v.get("firmwareVersion");
        let fs = |k: &str| {
            fw.and_then(|f| f.get(k))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };

        let mut associations = BTreeMap::new();
        if let Some(Value::Object(m)) = v.get("associations") {
            for (k, a) in m {
                let Some(a) = first_object(Some(a), id, file)? else {
                    continue;
                };
                let group: u8 = k
                    .parse()
                    .map_err(|_| err(file, format!("association group {k}")))?;
                associations.insert(
                    group,
                    AssociationConfig {
                        label: string(first_value(a.get("label"), id, file)?).unwrap_or_default(),
                        max_nodes: int(a.get("maxNodes")).unwrap_or(1).clamp(0, 255) as u8,
                        is_lifeline: a
                            .get("isLifeline")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        multi_channel: a.get("multiChannel").and_then(Value::as_bool),
                    },
                );
            }
        }

        // Parameters: the first applicable variant per "#", in file order.
        let mut params: Vec<ParamConfig> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        if let Some(Value::Array(list)) = v.get("paramInformation") {
            let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
            for p in list {
                let key = p
                    .get("#")
                    .and_then(Value::as_str)
                    .ok_or_else(|| err(file, "parameter without #"))?
                    .to_string();
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, g)) => g.push(p),
                    None => groups.push((key, vec![p])),
                }
            }
            for (key, variants) in groups {
                let mut chosen = None;
                for p in variants {
                    if let Some(x) = deep(p, id, file)? {
                        chosen = Some(x);
                        break;
                    }
                }
                let Some(p) = chosen else { continue };
                let (parameter, mask) = parse_param_key(&key)
                    .ok_or_else(|| err(file, format!("bad parameter key {key}")))?;
                let size = int(p.get("valueSize")).unwrap_or(0);
                let options: Vec<(i64, String)> = p
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|o| {
                                Some((int(o.get("value"))?, o.get("label")?.as_str()?.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let min = int(p.get("minValue"))
                    .or_else(|| options.iter().map(|o| o.0).min())
                    .unwrap_or(0);
                let max = int(p.get("maxValue"))
                    .or_else(|| options.iter().map(|o| o.0).max())
                    .unwrap_or(0);
                params.push(ParamConfig {
                    parameter,
                    mask,
                    key: key.clone(),
                    label: string(first_value(p.get("label"), id, file)?).unwrap_or_default(),
                    description: string(first_value(p.get("description"), id, file)?),
                    size: size.clamp(0, 4) as u8,
                    min,
                    max,
                    default: int(p.get("defaultValue")).unwrap_or(0),
                    unsigned: p.get("unsigned").and_then(Value::as_bool).unwrap_or(false),
                    read_only: p.get("readOnly").and_then(Value::as_bool).unwrap_or(false),
                    write_only: p.get("writeOnly").and_then(Value::as_bool).unwrap_or(false),
                    allow_manual_entry: p
                        .get("allowManualEntry")
                        .and_then(Value::as_bool)
                        .unwrap_or(options.is_empty()),
                    options,
                    unit: string(first_value(p.get("unit"), id, file)?),
                });
                seen.push(key);
            }
        }
        // Partial parameters take their size from the whole parameter's other parts.
        let sizes: HashMap<u16, u8> = params
            .iter()
            .filter(|p| p.size > 0)
            .map(|p| (p.parameter, p.size))
            .collect();
        for p in &mut params {
            if p.size == 0 {
                p.size = sizes.get(&p.parameter).copied().unwrap_or(0);
            }
        }
        let mut metadata =
            deep(v.get("metadata").unwrap_or(&Value::Null), id, file)?.filter(|m| !m.is_null());
        if let Some(Value::Object(m)) = metadata.as_mut() {
            let keys: Vec<String> = m.keys().cloned().collect();
            for k in keys {
                if let Some(Value::Array(_)) = m.get(&k) {
                    let raw = v.get("metadata").and_then(|x| x.get(&k)).cloned();
                    if let Some(val) = first_value(raw.as_ref(), id, file)? {
                        if !val.is_object() {
                            m.insert(k, val);
                        }
                    }
                }
            }
        }
        Ok(DeviceConfig {
            file: file.to_string(),
            manufacturer: s("manufacturer")?.unwrap_or_default(),
            manufacturer_id,
            label: s("label")?.unwrap_or_default(),
            description: s("description")?.unwrap_or_default(),
            devices,
            firmware: (fs("min"), fs("max")),
            associations,
            params,
            compat: first_object(v.get("compat"), id, file)?,
            endpoints: deep(v.get("endpoints").unwrap_or(&Value::Null), id, file)?
                .filter(|x| !x.is_null()),
            metadata,
        })
    }

    /// The parameter for a key (`"3"` or `"41[0xf00]"`).
    pub fn param(&self, key: &str) -> Option<&ParamConfig> {
        let (n, m) = parse_param_key(key)?;
        self.params.iter().find(|p| p.parameter == n && p.mask == m)
    }

    /// The lifeline group, if the file names one.
    pub fn lifeline(&self) -> Option<u8> {
        self.associations
            .iter()
            .find(|(_, a)| a.is_lifeline)
            .map(|(g, _)| *g)
    }
}

/// Why settings cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// The file has no such parameter.
    #[error("unknown parameter {0}")]
    Unknown(String),
    /// The parameter cannot be written.
    #[error("parameter {0} is read-only")]
    ReadOnly(String),
    /// Out of the parameter's range.
    #[error("{key}: {value} is outside {min}..={max}")]
    OutOfRange {
        /// Key.
        key: String,
        /// Value given.
        value: i64,
        /// Minimum.
        min: i64,
        /// Maximum.
        max: i64,
    },
    /// The parameter's size is unknown or not 1, 2 or 4.
    #[error("parameter {0} has no valid size")]
    Size(String),
}

/// One parameter write, partials merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamWrite {
    /// Parameter number.
    pub parameter: u16,
    /// Size in bytes.
    pub size: u8,
    /// The whole value, as the unsigned bit pattern of `size` bytes.
    pub raw: u32,
    /// Partial values that need the parameter's current value to merge into.
    pub needs_current: bool,
}

impl ParamWrite {
    /// The Configuration Set command (value as the signed integer the wire carries).
    pub fn command(&self) -> Command {
        let value = match self.size {
            1 => i32::from(self.raw as u8 as i8),
            2 => i32::from(self.raw as u16 as i16),
            _ => self.raw as i32,
        };
        configuration::Set {
            parameter: self.parameter as u8,
            default: false,
            size: self.size,
            value,
        }
        .into()
    }
}

/// Plans the writes for settings keyed as in the file (`{"1": 0, "41[0xf00]": 2}`).
///
/// `current` gives a parameter's current raw value when known; partial
/// parameters are merged into it (or into the other parts' defaults).
pub fn plan_settings(
    cfg: &DeviceConfig,
    settings: &[(String, i64)],
    current: &dyn Fn(u16) -> Option<u32>,
) -> Result<Vec<ParamWrite>, SettingsError> {
    let mut writes: BTreeMap<u16, ParamWrite> = BTreeMap::new();
    for (key, value) in settings {
        let p = cfg
            .param(key)
            .ok_or_else(|| SettingsError::Unknown(key.clone()))?;
        if p.read_only {
            return Err(SettingsError::ReadOnly(key.clone()));
        }
        if (p.min != 0 || p.max != 0) && !p.allow_manual_entry && !p.options.is_empty() {
            if !p.options.iter().any(|o| o.0 == *value) {
                return Err(SettingsError::OutOfRange {
                    key: key.clone(),
                    value: *value,
                    min: p.min,
                    max: p.max,
                });
            }
        } else if (p.min != 0 || p.max != 0) && (*value < p.min || *value > p.max) {
            return Err(SettingsError::OutOfRange {
                key: key.clone(),
                value: *value,
                min: p.min,
                max: p.max,
            });
        }
        if !matches!(p.size, 1 | 2 | 4) {
            return Err(SettingsError::Size(key.clone()));
        }
        let width_mask: u32 = if p.size == 4 {
            u32::MAX
        } else {
            (1u32 << (8 * u32::from(p.size))) - 1
        };
        match p.mask {
            None => {
                writes.insert(
                    p.parameter,
                    ParamWrite {
                        parameter: p.parameter,
                        size: p.size,
                        raw: (*value as u32) & width_mask,
                        needs_current: false,
                    },
                );
            }
            Some(mask) => {
                let w = writes.entry(p.parameter).or_insert_with(|| {
                    let base = current(p.parameter).unwrap_or_else(|| {
                        cfg.params
                            .iter()
                            .filter(|q| q.parameter == p.parameter)
                            .filter_map(|q| {
                                q.mask
                                    .map(|m| ((q.default as u32) << m.trailing_zeros()) & m)
                            })
                            .fold(0, |a, b| a | b)
                    });
                    ParamWrite {
                        parameter: p.parameter,
                        size: p.size,
                        raw: base & width_mask,
                        needs_current: current(p.parameter).is_none(),
                    }
                });
                let shifted = ((*value as u32) << mask.trailing_zeros()) & mask;
                w.raw = (w.raw & !mask) | shifted;
            }
        }
    }
    Ok(writes.into_values().collect())
}

/// The device ids of a node, from its interview.
pub fn device_id(rec: &crate::NodeRecord) -> Option<DeviceId> {
    let m = rec.manufacturer?;
    Some(DeviceId {
        manufacturer_id: m.manufacturer_id,
        product_type: m.product_type,
        product_id: m.product_id,
        firmware_version: rec.firmware.clone().unwrap_or_else(|| "0.0".into()),
        sdk_version: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(fw: &str) -> DeviceId {
        DeviceId {
            manufacturer_id: 0x027a,
            product_type: 0x2021,
            product_id: 0x2101,
            firmware_version: fw.into(),
            sdk_version: None,
        }
    }

    #[test]
    fn conditions() {
        let d = id("32.32");
        assert!(evaluate(
            "firmwareVersion >= 32.32 || firmwareVersion >= 1.10 && firmwareVersion < 16.9",
            &d
        )
        .unwrap());
        assert!(evaluate(
            "firmwareVersion >= 1.10 && firmwareVersion < 16.9",
            &id("1.10")
        )
        .unwrap());
        assert!(!evaluate("firmwareVersion >= 1.10", &id("1.9")).unwrap());
        assert!(evaluate("productType === 0x2021 && productId === 0x2101", &d).unwrap());
        assert!(!evaluate("sdkVersion >= 7.0", &d).unwrap());
        assert!(evaluate("!(productId === 0x0001)", &d).unwrap());
        assert!(evaluate("firmwareVersion = 1", &d).is_err());
        assert!(evaluate("bogus > 1", &d).is_err());
    }

    #[test]
    fn keys_and_versions() {
        assert_eq!(parse_param_key("41[0xf00]"), Some((41, Some(0xf00))));
        assert_eq!(parse_param_key("7"), Some((7, None)));
        assert_eq!(version_cmp("1.10", "1.9"), std::cmp::Ordering::Greater);
        assert_eq!(version_cmp("2.0", "2.0.0"), std::cmp::Ordering::Equal);
    }
}
