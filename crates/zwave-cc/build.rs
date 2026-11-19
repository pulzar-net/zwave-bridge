//! Generates the command class types from `defs/cc/*.toml` and the sensor,
//! meter and notification tables from `defs/tables/*.toml`.
//!
//! Each command becomes a struct with `parse` (bounded by the input, never
//! panicking) and `encode_payload`; each command class a `Command` enum; and
//! the crate a top-level `Command` that dispatches on the class id.

use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

#[derive(Deserialize)]
struct CcDef {
    id: u16,
    name: String,
    title: String,
    version: u8,
    #[serde(default)]
    command: Vec<CmdDef>,
}

#[derive(Deserialize)]
struct CmdDef {
    id: u8,
    name: String,
    #[serde(default)]
    fields: Vec<FieldDef>,
}

#[derive(Deserialize, Clone)]
struct FieldDef {
    name: Option<String>,
    #[serde(rename = "type")]
    ty: String,
    #[serde(default)]
    optional: bool,
    when: Option<String>,
    len: Option<String>,
    size: Option<String>,
    offset: Option<u16>,
    #[serde(default)]
    parts: Vec<PartDef>,
}

#[derive(Deserialize, Clone)]
struct PartDef {
    name: String,
    shift: u8,
    width: u8,
    kind: Option<String>,
}

fn camel(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn title_case(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rust type of a scalar field.
fn scalar_ty(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "u8" => "u8",
        "u16" => "u16",
        "u24" | "u32" => "u32",
        "i8" => "i8",
        "i16" => "i16",
        "i32" => "i32",
        "bool" => "bool",
        "value" => "crate::Value",
        "sized" => "i32",
        _ => return None,
    })
}

fn rest_ty(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "bytes" => "Vec<u8>",
        "bitmask" | "cc_list" => "Vec<u16>",
        "u8_list" => "Vec<u8>",
        _ => return None,
    })
}

fn reader_call(ty: &str) -> &'static str {
    match ty {
        "u8" => "r.u8()?",
        "u16" => "r.u16()?",
        "u24" => "r.u24()?",
        "u32" => "r.u32()?",
        "i8" => "r.i8()?",
        "i16" => "r.i16()?",
        "i32" => "r.i32()?",
        "bool" => "r.u8()? != 0",
        "value" => "r.value()?",
        _ => unreachable!(),
    }
}

fn write_call(ty: &str, expr: &str) -> String {
    match ty {
        "u8" | "i8" => format!("w.push({expr} as u8);"),
        "u16" | "i16" => format!("w.extend_from_slice(&({expr} as u16).to_be_bytes());"),
        "u24" => format!("w.extend_from_slice(&({expr} as u32).to_be_bytes()[1..]);"),
        "u32" | "i32" => format!("w.extend_from_slice(&({expr} as u32).to_be_bytes());"),
        "bool" => format!("w.push(if {expr} {{ 0xff }} else {{ 0 }});"),
        "value" => format!("{expr}.encode(w);"),
        _ => unreachable!(),
    }
}

/// What a name refers to, for `when`, `len` and `size` lookups.
#[derive(Clone)]
struct Local {
    ty: String,
    optional: bool,
}

fn gen_command(out: &mut String, cc: &CcDef, cmd: &CmdDef) {
    let sname = camel(&cmd.name);
    let derived: HashSet<String> = cmd.fields.iter().filter_map(|f| f.len.clone()).collect();
    // Struct
    writeln!(
        out,
        "    /// {} {} (`{:#04x} {:#04x}`).",
        cc.title,
        title_case(&cmd.name),
        cc.id,
        cmd.id
    )
    .unwrap();
    writeln!(out, "    #[derive(Debug, Clone, PartialEq, Eq, Default)]").unwrap();
    writeln!(out, "    pub struct {sname} {{").unwrap();
    for f in &cmd.fields {
        if f.ty == "bits" {
            for p in &f.parts {
                if derived.contains(&p.name) {
                    continue;
                }
                let t = if p.kind.as_deref() == Some("bool") {
                    "bool"
                } else {
                    "u8"
                };
                let t = if f.optional {
                    format!("Option<{t}>")
                } else {
                    t.to_string()
                };
                writeln!(
                    out,
                    "        /// The `{}` field (bits {}..{}).",
                    p.name,
                    p.shift,
                    p.shift + p.width
                )
                .unwrap();
                writeln!(out, "        pub {}: {t},", p.name).unwrap();
            }
            continue;
        }
        let name = f.name.as_ref().expect("named field");
        if derived.contains(name) {
            continue;
        }
        let t = if let Some(t) = scalar_ty(&f.ty) {
            if f.optional {
                format!("Option<{t}>")
            } else {
                t.to_string()
            }
        } else {
            rest_ty(&f.ty)
                .unwrap_or_else(|| panic!("unknown type {}", f.ty))
                .to_string()
        };
        writeln!(out, "        /// The `{name}` field.").unwrap();
        writeln!(out, "        pub {name}: {t},").unwrap();
    }
    writeln!(out, "    }}\n").unwrap();

    // Parse
    let mut locals: BTreeMap<String, Local> = BTreeMap::new();
    let mut parse = String::new();
    let mut seen_optional = false;
    for (i, f) in cmd.fields.iter().enumerate() {
        if f.optional {
            seen_optional = true;
        } else if seen_optional && scalar_ty(&f.ty).is_some() && f.ty != "bits" {
            panic!(
                "{}.{}: required field after an optional one",
                cc.name, cmd.name
            );
        }
        let cond = f.when.as_ref().map(|w| {
            let l = locals.get(w).unwrap_or_else(|| panic!("when {w} unknown"));
            let v = format!("f_{w}");
            match (l.ty.as_str(), l.optional) {
                ("bool", true) => format!("{v}.unwrap_or(false)"),
                ("bool", false) => v,
                (_, true) => format!("{v}.unwrap_or(0) != 0"),
                (_, false) => format!("{v} != 0"),
            }
        });
        if f.ty == "bits" {
            let b = format!("b{i}");
            if f.optional {
                writeln!(
                    parse,
                    "        let {b}: Option<u8> = if r.has() {{ Some(r.u8()?) }} else {{ None }};"
                )
                .unwrap();
            } else {
                writeln!(parse, "        let {b}: u8 = r.u8()?;").unwrap();
            }
            for p in &f.parts {
                let mask = (1u16 << p.width) - 1;
                let is_bool = p.kind.as_deref() == Some("bool");
                let extract = if is_bool {
                    format!("(x >> {}) & {mask:#x} != 0", p.shift)
                } else {
                    format!("(x >> {}) & {mask:#x}", p.shift)
                };
                let d = derived.contains(&p.name);
                let line = match (f.optional, d) {
                    (true, true) => format!(
                        "let f_{}: u8 = {b}.map(|x| {extract}).unwrap_or(0);",
                        p.name
                    ),
                    (true, false) => format!("let f_{} = {b}.map(|x| {extract});", p.name),
                    (false, _) => format!("let f_{} = {{ let x = {b}; {extract} }};", p.name),
                };
                writeln!(parse, "        {line}").unwrap();
                locals.insert(
                    p.name.clone(),
                    Local {
                        ty: if is_bool { "bool".into() } else { "u8".into() },
                        optional: f.optional && !d,
                    },
                );
            }
            continue;
        }
        let name = f.name.clone().unwrap();
        let expr = match f.ty.as_str() {
            "sized" => {
                let s = f.size.as_ref().expect("size");
                let l = locals.get(s).unwrap_or_else(|| panic!("size {s} unknown"));
                let n = if l.ty == "value" {
                    if l.optional {
                        format!("f_{s}.map(|v| v.size).unwrap_or(0)")
                    } else {
                        format!("f_{s}.size")
                    }
                } else if l.optional {
                    format!("f_{s}.unwrap_or(0)")
                } else {
                    format!("f_{s}")
                };
                format!("r.sized({n})?")
            }
            "bytes" | "bitmask" => {
                let raw = match &f.len {
                    Some(l) => format!("r.take(usize::from(f_{l}))?"),
                    None => "r.rest()".to_string(),
                };
                if f.ty == "bytes" {
                    format!("{raw}.to_vec()")
                } else {
                    format!("crate::decode_bitmask({raw}, {})", f.offset.unwrap_or(0))
                }
            }
            "cc_list" => "crate::decode_cc_list(r.rest())".into(),
            "u8_list" => "r.rest().to_vec()".into(),
            t => reader_call(t).to_string(),
        };
        let is_rest = rest_ty(&f.ty).is_some();
        let line = if is_rest {
            format!("let f_{name} = {expr};")
        } else if f.optional {
            let c = cond
                .clone()
                .map(|c| format!("({c}) && "))
                .unwrap_or_default();
            format!("let f_{name} = if {c}r.has() {{ Some({expr}) }} else {{ None }};")
        } else {
            format!("let f_{name} = {expr};")
        };
        writeln!(parse, "        {line}").unwrap();
        locals.insert(
            name,
            Local {
                ty: f.ty.clone(),
                optional: f.optional && !is_rest,
            },
        );
    }

    // Encode
    let mut enc = String::new();
    let field_expr = |n: &str| format!("self.{n}");
    let len_expr = |target: &FieldDef| -> String {
        let n = target.name.as_ref().unwrap();
        match target.ty.as_str() {
            "bitmask" => format!("mask_{n}.len()"),
            _ => format!("self.{n}.len()"),
        }
    };
    let len_target = |name: &str| -> FieldDef {
        cmd.fields
            .iter()
            .find(|f| f.len.as_deref() == Some(name))
            .cloned()
            .unwrap()
    };
    for f in &cmd.fields {
        if f.ty == "bitmask" {
            let n = f.name.as_ref().unwrap();
            writeln!(
                enc,
                "        let mask_{n} = crate::encode_bitmask(&self.{n}, {});",
                f.offset.unwrap_or(0)
            )
            .unwrap();
        }
    }
    let enc_start = enc.len();
    for (i, f) in cmd.fields.iter().enumerate() {
        let cond = f.when.as_ref().map(|w| {
            let l = locals.get(w).unwrap();
            match (l.ty.as_str(), l.optional) {
                ("bool", true) => format!("self.{w}.unwrap_or(false)"),
                ("bool", false) => format!("self.{w}"),
                (_, true) => format!("self.{w}.unwrap_or(0) != 0"),
                (_, false) => format!("self.{w} != 0"),
            }
        });
        if f.ty == "bits" {
            let mut terms = Vec::new();
            let mut present = Vec::new();
            for p in &f.parts {
                let mask = (1u16 << p.width) - 1;
                let is_bool = p.kind.as_deref() == Some("bool");
                let v = if derived.contains(&p.name) {
                    let t = len_target(&p.name);
                    present.push(format!("{} != 0", len_expr(&t)));
                    format!("({} as u8)", len_expr(&t))
                } else if f.optional {
                    present.push(format!("self.{}.is_some()", p.name));
                    if is_bool {
                        format!("u8::from(self.{}.unwrap_or(false))", p.name)
                    } else {
                        format!("self.{}.unwrap_or(0)", p.name)
                    }
                } else if is_bool {
                    format!("u8::from(self.{})", p.name)
                } else {
                    format!("self.{}", p.name)
                };
                terms.push(format!("({v} & {mask:#x}) << {}", p.shift));
            }
            if f.optional {
                writeln!(
                    enc,
                    "            if !({}) {{ break 'enc; }}",
                    present.join(" || ")
                )
                .unwrap();
            }
            let joined = if terms.len() == 1 {
                terms[0].clone()
            } else {
                terms
                    .iter()
                    .map(|t| format!("({t})"))
                    .collect::<Vec<_>>()
                    .join(" | ")
            };
            writeln!(enc, "            let b{i}: u8 = {joined};").unwrap();
            writeln!(enc, "            w.push(b{i});").unwrap();
            continue;
        }
        let name = f.name.clone().unwrap();
        if derived.contains(&name) {
            let t = len_target(&name);
            writeln!(enc, "            w.push({} as u8);", len_expr(&t)).unwrap();
            continue;
        }
        let body = match f.ty.as_str() {
            "sized" => {
                let s = f.size.as_ref().unwrap();
                let l = locals.get(s).unwrap();
                let n = if l.ty == "value" {
                    format!("self.{s}.size")
                } else if l.optional {
                    format!("self.{s}.unwrap_or(0)")
                } else {
                    format!("self.{s}")
                };
                if f.optional {
                    format!("crate::write_sized(w, v, {n});")
                } else {
                    format!("crate::write_sized(w, self.{name}, {n});")
                }
            }
            "bytes" | "u8_list" => format!("w.extend_from_slice(&self.{name});"),
            "bitmask" => format!("w.extend_from_slice(&mask_{name});"),
            "cc_list" => format!("crate::encode_cc_list(&self.{name}, w);"),
            t if f.optional => write_call(t, "v"),
            t => write_call(t, &field_expr(&name)),
        };
        if f.optional && rest_ty(&f.ty).is_none() {
            let inner =
                format!("match self.{name} {{ Some(v) => {{ {body} }} None => break 'enc }}");
            match cond {
                Some(c) => writeln!(enc, "            if {c} {{ {inner} }}").unwrap(),
                None => writeln!(enc, "            {inner}").unwrap(),
            }
        } else {
            writeln!(enc, "            {body}").unwrap();
        }
    }
    if enc.contains("break 'enc") {
        enc.insert_str(enc_start, "        'enc: {\n");
        enc.push_str("        }\n");
    }

    let fields: Vec<String> = cmd
        .fields
        .iter()
        .flat_map(|f| {
            if f.ty == "bits" {
                f.parts.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
            } else {
                vec![f.name.clone().unwrap()]
            }
        })
        .filter(|n| !derived.contains(n))
        .collect();
    let init = fields
        .iter()
        .map(|n| format!("{n}: f_{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(out, "    impl {sname} {{").unwrap();
    writeln!(
        out,
        "        /// Command class id.\n        pub const CC: u16 = {:#04x};",
        cc.id
    )
    .unwrap();
    writeln!(
        out,
        "        /// Command id.\n        pub const ID: u8 = {:#04x};",
        cmd.id
    )
    .unwrap();
    writeln!(out, "        /// Parses the payload after the command id. Extra trailing bytes (newer versions) are ignored.").unwrap();
    writeln!(out, "        #[allow(unused_mut, unused_variables, clippy::double_parens, clippy::nonminimal_bool)]").unwrap();
    writeln!(
        out,
        "        pub fn parse(p: &[u8]) -> Result<Self, crate::ParseError> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let mut r = crate::reader::Reader::new(p, Self::CC, Self::ID);"
    )
    .unwrap();
    out.push_str(&parse);
    writeln!(out, "        Ok(Self {{ {init} }})\n        }}").unwrap();
    writeln!(out, "        /// Appends the payload after the command id.").unwrap();
    writeln!(out, "        #[allow(unused_variables, clippy::identity_op, clippy::unnecessary_cast, clippy::double_parens, clippy::bool_to_int_with_if)]").unwrap();
    writeln!(
        out,
        "        pub fn encode_payload(&self, w: &mut Vec<u8>) {{"
    )
    .unwrap();
    out.push_str(&enc);
    writeln!(out, "        }}\n    }}\n").unwrap();
    writeln!(out, "    impl From<{sname}> for crate::Command {{\n        fn from(c: {sname}) -> Self {{ crate::Command::{}(Command::{sname}(c)) }}\n    }}\n", camel(&cc.name)).unwrap();
}

fn gen_cc(out: &mut String, cc: &CcDef) {
    writeln!(
        out,
        "/// {} command class (`{:#04x}`), version {}.",
        cc.title, cc.id, cc.version
    )
    .unwrap();
    writeln!(out, "pub mod {} {{", cc.name).unwrap();
    writeln!(
        out,
        "    /// Command class id.\n    pub const ID: u16 = {:#04x};",
        cc.id
    )
    .unwrap();
    writeln!(
        out,
        "    /// Highest version these definitions cover.\n    pub const VERSION: u8 = {};\n",
        cc.version
    )
    .unwrap();
    for cmd in &cc.command {
        gen_command(out, cc, cmd);
    }
    writeln!(out, "    /// Every {} command.", cc.title).unwrap();
    writeln!(
        out,
        "    #[derive(Debug, Clone, PartialEq, Eq)]\n    pub enum Command {{"
    )
    .unwrap();
    for cmd in &cc.command {
        writeln!(
            out,
            "        /// {}.\n        {}({}),",
            title_case(&cmd.name),
            camel(&cmd.name),
            camel(&cmd.name)
        )
        .unwrap();
    }
    writeln!(out, "    }}\n\n    impl Command {{").unwrap();
    writeln!(
        out,
        "        /// Parses a command of this class; `Ok(None)` for an unknown command id."
    )
    .unwrap();
    writeln!(out, "        pub fn parse(command: u8, payload: &[u8]) -> Result<Option<Self>, crate::ParseError> {{\n            Ok(Some(match command {{").unwrap();
    for cmd in &cc.command {
        writeln!(
            out,
            "                {:#04x} => Command::{}({}::parse(payload)?),",
            cmd.id,
            camel(&cmd.name),
            camel(&cmd.name)
        )
        .unwrap();
    }
    writeln!(
        out,
        "                _ => return Ok(None),\n            }}))\n        }}"
    )
    .unwrap();
    writeln!(out, "        /// The command id.\n        pub fn command_id(&self) -> u8 {{\n            match self {{").unwrap();
    for cmd in &cc.command {
        writeln!(
            out,
            "                Command::{}(_) => {:#04x},",
            camel(&cmd.name),
            cmd.id
        )
        .unwrap();
    }
    writeln!(out, "            }}\n        }}").unwrap();
    writeln!(out, "        /// The command's name.\n        pub fn name(&self) -> &'static str {{\n            match self {{").unwrap();
    for cmd in &cc.command {
        writeln!(
            out,
            "                Command::{}(_) => \"{}\",",
            camel(&cmd.name),
            cmd.name
        )
        .unwrap();
    }
    writeln!(out, "            }}\n        }}").unwrap();
    writeln!(out, "        /// Appends the payload after the command id.\n        pub fn encode_payload(&self, w: &mut Vec<u8>) {{\n            match self {{").unwrap();
    for cmd in &cc.command {
        writeln!(
            out,
            "                Command::{}(c) => c.encode_payload(w),",
            camel(&cmd.name)
        )
        .unwrap();
    }
    writeln!(out, "            }}\n        }}\n    }}\n}}\n").unwrap();
}

fn gen_top(out: &mut String, ccs: &[CcDef]) {
    writeln!(out, "/// A parsed command of any known command class.").unwrap();
    writeln!(
        out,
        "#[derive(Debug, Clone, PartialEq, Eq)]\npub enum Command {{"
    )
    .unwrap();
    writeln!(
        out,
        "    /// No Operation (`0x00`), a ping with no command id.\n    NoOperation,"
    )
    .unwrap();
    for cc in ccs {
        writeln!(
            out,
            "    /// {}.\n    {}({}::Command),",
            cc.title,
            camel(&cc.name),
            cc.name
        )
        .unwrap();
    }
    writeln!(out, "    /// A command class or command these definitions do not cover.\n    Unknown(crate::RawCommand),\n}}\n").unwrap();
    writeln!(out, "impl Command {{").unwrap();
    writeln!(out, "    pub(crate) fn parse_known(cc: u16, command: u8, payload: &[u8]) -> Result<Option<Self>, crate::ParseError> {{\n        Ok(match cc {{").unwrap();
    for cc in ccs {
        writeln!(
            out,
            "            {:#04x} => {}::Command::parse(command, payload)?.map(Command::{}),",
            cc.id,
            cc.name,
            camel(&cc.name)
        )
        .unwrap();
    }
    writeln!(out, "            _ => None,\n        }})\n    }}").unwrap();
    writeln!(out, "    /// The command class id.\n    pub fn cc(&self) -> u16 {{\n        match self {{\n            Command::NoOperation => 0,").unwrap();
    for cc in ccs {
        writeln!(
            out,
            "            Command::{}(_) => {:#04x},",
            camel(&cc.name),
            cc.id
        )
        .unwrap();
    }
    writeln!(
        out,
        "            Command::Unknown(r) => r.cc,\n        }}\n    }}"
    )
    .unwrap();
    writeln!(out, "    /// The command id (0 for No Operation).\n    pub fn command_id(&self) -> u8 {{\n        match self {{\n            Command::NoOperation => 0,").unwrap();
    for cc in ccs {
        writeln!(
            out,
            "            Command::{}(c) => c.command_id(),",
            camel(&cc.name)
        )
        .unwrap();
    }
    writeln!(
        out,
        "            Command::Unknown(r) => r.command,\n        }}\n    }}"
    )
    .unwrap();
    writeln!(out, "    /// `class.command`, e.g. `multilevel_sensor.report`.\n    pub fn name(&self) -> String {{\n        match self {{\n            Command::NoOperation => \"no_operation\".into(),").unwrap();
    for cc in ccs {
        writeln!(
            out,
            "            Command::{}(c) => format!(\"{}.{{}}\", c.name()),",
            camel(&cc.name),
            cc.name
        )
        .unwrap();
    }
    writeln!(out, "            Command::Unknown(r) => format!(\"{{:#06x}}.{{:#04x}}\", r.cc, r.command),\n        }}\n    }}").unwrap();
    writeln!(out, "    pub(crate) fn encode_payload(&self, w: &mut Vec<u8>) {{\n        match self {{\n            Command::NoOperation => {{}}").unwrap();
    for cc in ccs {
        writeln!(
            out,
            "            Command::{}(c) => c.encode_payload(w),",
            camel(&cc.name)
        )
        .unwrap();
    }
    writeln!(out, "            Command::Unknown(r) => w.extend_from_slice(&r.payload),\n        }}\n    }}\n}}\n").unwrap();
    writeln!(
        out,
        "/// Every command class these definitions cover: id, name, title, version."
    )
    .unwrap();
    writeln!(out, "pub const COMMAND_CLASSES: &[crate::CcInfo] = &[").unwrap();
    for cc in ccs {
        writeln!(
            out,
            "    crate::CcInfo {{ id: {:#04x}, name: \"{}\", title: \"{}\", version: {} }},",
            cc.id, cc.name, cc.title, cc.version
        )
        .unwrap();
    }
    writeln!(out, "];").unwrap();
}

#[derive(Deserialize)]
struct ScaleDef {
    id: u8,
    label: String,
    unit: String,
}
#[derive(Deserialize)]
struct TypeDef {
    id: u8,
    name: String,
    label: String,
    #[serde(default)]
    scales: Vec<ScaleDef>,
    #[serde(default)]
    events: Vec<EventDef>,
}
#[derive(Deserialize)]
struct EventDef {
    id: u8,
    label: String,
    variable: String,
    idle: bool,
}
#[derive(Deserialize)]
struct Tables {
    #[serde(default)]
    sensor: Vec<TypeDef>,
    #[serde(default)]
    meter: Vec<TypeDef>,
    #[serde(default)]
    notification: Vec<TypeDef>,
}

fn gen_tables(out: &mut String, dir: &Path) {
    let load = |f: &str| -> Tables {
        let p = dir.join(f);
        println!("cargo:rerun-if-changed={}", p.display());
        toml::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap_or_else(|e| panic!("{f}: {e}"))
    };
    let mut sensors = load("sensor_types.toml").sensor;
    let mut meters = load("meter_types.toml").meter;
    let mut notes = load("notification_types.toml").notification;
    for (list, ty, name) in [
        (&mut sensors, "SensorType", "SENSOR_TYPES"),
        (&mut meters, "MeterType", "METER_TYPES"),
    ] {
        list.sort_by_key(|t| t.id);
        writeln!(
            out,
            "/// Every assigned {}, by id.\npub static {name}: &[{ty}] = &[",
            if ty == "SensorType" {
                "multilevel sensor type"
            } else {
                "meter type"
            }
        )
        .unwrap();
        for t in list.iter() {
            write!(
                out,
                "    {ty} {{ id: {}, name: {:?}, label: {:?}, scales: &[",
                t.id, t.name, t.label
            )
            .unwrap();
            for s in &t.scales {
                write!(
                    out,
                    "Scale {{ id: {}, label: {:?}, unit: {:?} }}, ",
                    s.id, s.label, s.unit
                )
                .unwrap();
            }
            writeln!(out, "] }},").unwrap();
        }
        writeln!(out, "];\n").unwrap();
    }
    notes.sort_by_key(|t| t.id);
    writeln!(out, "/// Every assigned notification type, by id.\npub static NOTIFICATION_TYPES: &[NotificationType] = &[").unwrap();
    for t in &notes {
        write!(
            out,
            "    NotificationType {{ id: {}, name: {:?}, label: {:?}, events: &[",
            t.id, t.name, t.label
        )
        .unwrap();
        let mut evs: Vec<&EventDef> = t.events.iter().collect();
        evs.sort_by_key(|e| e.id);
        for e in evs {
            write!(
                out,
                "NotificationEvent {{ id: {}, label: {:?}, variable: {:?}, idle: {} }}, ",
                e.id, e.label, e.variable, e.idle
            )
            .unwrap();
        }
        writeln!(out, "] }},").unwrap();
    }
    writeln!(out, "];").unwrap();
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("defs");
    let cc_dir = root.join("cc");
    println!("cargo:rerun-if-changed={}", cc_dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&cc_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    let mut ccs: Vec<CcDef> = files
        .iter()
        .map(|p| {
            println!("cargo:rerun-if-changed={}", p.display());
            toml::from_str(&std::fs::read_to_string(p).unwrap())
                .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        })
        .collect();
    ccs.sort_by_key(|c| c.id);
    let mut ids = HashSet::new();
    for cc in &ccs {
        assert!(ids.insert(cc.id), "duplicate command class {:#x}", cc.id);
        let mut cmds = HashSet::new();
        for c in &cc.command {
            assert!(
                cmds.insert(c.id),
                "{}: duplicate command {:#x}",
                cc.name,
                c.id
            );
        }
    }
    let mut out = String::from("// @generated by build.rs from defs/cc/*.toml. Do not edit.\n\n");
    for cc in &ccs {
        gen_cc(&mut out, cc);
    }
    gen_top(&mut out, &ccs);
    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out_dir).join("generated.rs"), out).unwrap();

    let mut t = String::from("// @generated by build.rs from defs/tables/*.toml. Do not edit.\n\n");
    gen_tables(&mut t, &root.join("tables"));
    std::fs::write(Path::new(&out_dir).join("tables.rs"), t).unwrap();
}
