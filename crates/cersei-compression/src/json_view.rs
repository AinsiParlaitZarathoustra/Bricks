//! Structured summaries of large JSON documents.
//!
//! The summary is an *envelope* that is never mixed with the data: the
//! document's shape (keys, types, array lengths), a sample of verbatim values
//! (the first three items of each array by default) and a separate list of
//! everything the sample leaves out, by JSON Pointer. No marker key is ever
//! inserted into the data, so a document whose own keys look like the
//! envelope's cannot be confused with it.
//!
//! The document is scanned once by a small streaming parser that borrows from
//! the input: numbers and strings are copied as written (exact digits and
//! escapes, so types and values are preserved), only the sampled parts are
//! retained, and depth, input size and output size are bounded. Invalid JSON
//! is reported with its line and column; nothing is reconstructed.

/// Limits of a summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonViewOptions {
    /// Items sampled from each array.
    pub sample_items: usize,
    /// Nesting depth below which sampled containers are emptied.
    pub max_depth: usize,
    /// Keys sampled per object (and tracked per shape node).
    pub max_keys: usize,
    /// Characters kept from a sampled string.
    pub max_string_chars: usize,
    /// Upper bound of the rendered summary, in bytes.
    pub max_output_bytes: usize,
    /// Documents larger than this are not parsed.
    pub max_input_bytes: usize,
    /// Depth of the shape description.
    pub max_shape_depth: usize,
}

impl Default for JsonViewOptions {
    fn default() -> Self {
        Self {
            sample_items: 3,
            max_depth: 6,
            max_keys: 40,
            max_string_chars: 200,
            max_output_bytes: 12_000,
            max_input_bytes: 32 * 1024 * 1024,
            max_shape_depth: 6,
        }
    }
}

/// Why a document could not be summarised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonViewError {
    Invalid {
        line: usize,
        column: usize,
        message: String,
    },
    TooLarge {
        bytes: usize,
        limit: usize,
    },
}

impl std::fmt::Display for JsonViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonViewError::Invalid {
                line,
                column,
                message,
            } => {
                write!(f, "invalid JSON at line {line}, column {column}: {message}")
            }
            JsonViewError::TooLarge { bytes, limit } => {
                write!(
                    f,
                    "document of {bytes} bytes exceeds the {limit}-byte limit for a summary"
                )
            }
        }
    }
}

/// A rendered summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonSummary {
    pub text: String,
    pub root_type: &'static str,
    pub omissions: usize,
}

/// Does this text look like a JSON document (object or array)?
pub fn looks_like_json(text: &str) -> bool {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.starts_with('{') || t.starts_with('[')
}

/// Summarise `text`. `raw` says where the complete document can be read.
pub fn summarize(
    text: &str,
    opts: &JsonViewOptions,
    raw: &str,
) -> Result<JsonSummary, JsonViewError> {
    if text.len() > opts.max_input_bytes {
        return Err(JsonViewError::TooLarge {
            bytes: text.len(),
            limit: opts.max_input_bytes,
        });
    }
    // Progressively smaller views until the output fits.
    let mut o = opts.clone();
    for attempt in 0..5 {
        let doc = parse(text, &o)?;
        let rendered = render(&doc, text.len(), raw, &o, attempt == 4);
        if rendered.len() <= opts.max_output_bytes || attempt == 4 {
            return Ok(JsonSummary {
                text: rendered,
                root_type: doc.root_type,
                omissions: doc.omitted.len(),
            });
        }
        o.sample_items = o.sample_items.saturating_sub(1).max(1);
        o.max_string_chars = (o.max_string_chars / 2).max(16);
        o.max_keys = (o.max_keys / 2).max(5);
        o.max_depth = o.max_depth.saturating_sub(1).max(2);
        o.max_shape_depth = o.max_shape_depth.saturating_sub(1).max(2);
    }
    unreachable!("the last attempt always returns")
}

// ─── Parsed form ─────────────────────────────────────────────────────────────

const T_NULL: u8 = 1;
const T_BOOL: u8 = 2;
const T_NUMBER: u8 = 4;
const T_STRING: u8 = 8;
const T_ARRAY: u8 = 16;
const T_OBJECT: u8 = 32;
const MAX_NESTING: usize = 512;

#[derive(Debug, Default)]
struct Shape {
    types: u8,
    keys: Vec<(String, Shape)>,
    untracked_keys: bool,
    items: Option<Box<Shape>>,
    min_len: Option<u64>,
    max_len: u64,
    deeper: bool,
}

#[derive(Debug)]
enum Sample<'a> {
    /// A scalar copied verbatim (`12.50`, `true`, `"x\n"`).
    Raw(&'a str),
    /// A string cut to its first characters (still a valid JSON string).
    Cut(String),
    Array(Vec<Sample<'a>>),
    Object(Vec<(&'a str, Sample<'a>)>),
}

#[derive(Debug)]
struct Omitted {
    pointer: String,
    what: String,
}

struct Doc<'a> {
    root_type: &'static str,
    shape: Shape,
    sample: Option<Sample<'a>>,
    omitted: Vec<Omitted>,
}

struct Parser<'a, 'o> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    o: &'o JsonViewOptions,
    omitted: Vec<Omitted>,
}

fn parse<'a>(text: &'a str, o: &JsonViewOptions) -> Result<Doc<'a>, JsonViewError> {
    let mut p = Parser {
        s: text,
        b: text.as_bytes(),
        i: 0,
        o,
        omitted: Vec::new(),
    };
    if p.s.starts_with('\u{feff}') {
        p.i = 3;
    }
    p.ws();
    let mut shape = Shape::default();
    let mut pointer = String::new();
    let sample = p.value(0, Some(&mut shape), true, &mut pointer)?;
    p.ws();
    if p.i < p.b.len() {
        return Err(p.err("unexpected data after the document"));
    }
    let root_type = type_name(shape.types);
    Ok(Doc {
        root_type,
        shape,
        sample,
        omitted: p.omitted,
    })
}

impl<'a> Parser<'a, '_> {
    fn err(&self, message: &str) -> JsonViewError {
        let upto = &self.b[..self.i.min(self.b.len())];
        let line = upto.iter().filter(|&&c| c == b'\n').count() + 1;
        let col_start = upto
            .iter()
            .rposition(|&c| c == b'\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        let column = self.s[col_start..self.i.min(self.s.len())].chars().count() + 1;
        JsonViewError::Invalid {
            line,
            column,
            message: message.to_string(),
        }
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn omit(&mut self, pointer: &str, what: String) {
        // The list itself is bounded; its length is what matters past that.
        if self.omitted.len() < 200 {
            self.omitted.push(Omitted {
                pointer: pointer.to_string(),
                what,
            });
        }
    }

    /// Parse one value. `shape` is `None` below the shape depth; `sample`
    /// says whether this value is part of the sample.
    fn value(
        &mut self,
        depth: usize,
        shape: Option<&mut Shape>,
        sample: bool,
        pointer: &mut String,
    ) -> Result<Option<Sample<'a>>, JsonViewError> {
        if depth > MAX_NESTING {
            return Err(self.err("nesting deeper than 512 levels"));
        }
        match self.b.get(self.i) {
            Some(b'{') => self.object(depth, shape, sample, pointer),
            Some(b'[') => self.array(depth, shape, sample, pointer),
            Some(b'"') => {
                let start = self.i;
                self.string()?;
                let raw = &self.s[start..self.i];
                if let Some(sh) = shape {
                    sh.types |= T_STRING;
                }
                if !sample {
                    return Ok(None);
                }
                Ok(Some(self.cut_string(raw, pointer)))
            }
            Some(b't') => self.literal("true", T_BOOL, shape, sample),
            Some(b'f') => self.literal("false", T_BOOL, shape, sample),
            Some(b'n') => self.literal("null", T_NULL, shape, sample),
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                self.number()?;
                if let Some(sh) = shape {
                    sh.types |= T_NUMBER;
                }
                Ok(sample.then(|| Sample::Raw(&self.s[start..self.i])))
            }
            Some(_) => Err(self.err("expected a value")),
            None => Err(self.err("unexpected end of input")),
        }
    }

    fn literal(
        &mut self,
        word: &'static str,
        t: u8,
        shape: Option<&mut Shape>,
        sample: bool,
    ) -> Result<Option<Sample<'a>>, JsonViewError> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            let start = self.i;
            self.i += word.len();
            if let Some(sh) = shape {
                sh.types |= t;
            }
            Ok(sample.then(|| Sample::Raw(&self.s[start..self.i])))
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn number(&mut self) -> Result<(), JsonViewError> {
        let b = self.b;
        if b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        match b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(c) if c.is_ascii_digit() => {
                while b.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                }
            }
            _ => return Err(self.err("invalid number")),
        }
        if b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if !b.get(self.i).is_some_and(u8::is_ascii_digit) {
                return Err(self.err("invalid number: digit expected after `.`"));
            }
            while b.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        if matches!(b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !b.get(self.i).is_some_and(u8::is_ascii_digit) {
                return Err(self.err("invalid number: digit expected in exponent"));
            }
            while b.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        Ok(())
    }

    /// Skip a string, validating escapes. `self.i` is on the opening quote.
    fn string(&mut self) -> Result<(), JsonViewError> {
        self.i += 1;
        loop {
            match self.b.get(self.i) {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(());
                }
                Some(b'\\') => match self.b.get(self.i + 1) {
                    Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => self.i += 2,
                    Some(b'u') => {
                        let hex = self.b.get(self.i + 2..self.i + 6);
                        if !hex.is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit)) {
                            return Err(self.err("invalid \\u escape"));
                        }
                        self.i += 6;
                    }
                    _ => return Err(self.err("invalid escape")),
                },
                Some(c) if *c < 0x20 => return Err(self.err("control character in string")),
                Some(_) => self.i += 1,
            }
        }
    }

    /// A sampled string, cut to `max_string_chars` characters of its raw
    /// form without splitting an escape or a UTF-8 sequence.
    fn cut_string(&mut self, raw: &'a str, pointer: &str) -> Sample<'a> {
        let inner = &raw[1..raw.len() - 1];
        let limit = self.o.max_string_chars;
        let mut chars = 0usize;
        let mut end = None;
        let bytes = inner.as_bytes();
        let mut k = 0;
        while k < bytes.len() {
            if chars == limit {
                end = Some(k);
                break;
            }
            let step = if bytes[k] == b'\\' {
                if bytes.get(k + 1) == Some(&b'u') {
                    6
                } else {
                    2
                }
            } else {
                inner[k..].chars().next().map(char::len_utf8).unwrap_or(1)
            };
            k += step;
            chars += 1;
        }
        match end {
            None => Sample::Raw(raw),
            Some(e) => {
                let total = decoded_len(inner);
                self.omit(
                    pointer,
                    format!("string cut to its first {limit} of {total} characters"),
                );
                Sample::Cut(format!("\"{}\"", &inner[..e]))
            }
        }
    }

    fn array(
        &mut self,
        depth: usize,
        mut shape: Option<&mut Shape>,
        sample: bool,
        pointer: &mut String,
    ) -> Result<Option<Sample<'a>>, JsonViewError> {
        self.i += 1;
        let deep = depth >= self.o.max_shape_depth;
        if let Some(sh) = shape.as_deref_mut() {
            sh.types |= T_ARRAY;
            if deep {
                sh.deeper = true;
            }
        }
        let sample_children = sample && depth < self.o.max_depth;
        let mut items = Vec::new();
        let mut n: u64 = 0;
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
        } else {
            loop {
                self.ws();
                let take = sample_children && (n as usize) < self.o.sample_items;
                let len_before = pointer.len();
                pointer.push('/');
                pointer.push_str(&n.to_string());
                let child_shape = match shape.as_deref_mut() {
                    Some(sh) if !deep => Some(&mut **sh.items.get_or_insert_with(Default::default)),
                    _ => None,
                };
                let v = self.value(depth + 1, child_shape, take, pointer)?;
                pointer.truncate(len_before);
                if let Some(v) = v {
                    items.push(v);
                }
                n += 1;
                self.ws();
                match self.b.get(self.i) {
                    Some(b',') => self.i += 1,
                    Some(b']') => {
                        self.i += 1;
                        break;
                    }
                    _ => return Err(self.err("expected `,` or `]` in array")),
                }
            }
        }
        if let Some(sh) = shape {
            sh.min_len = Some(sh.min_len.map_or(n, |m| m.min(n)));
            sh.max_len = sh.max_len.max(n);
        }
        if !sample {
            return Ok(None);
        }
        let kept = items.len() as u64;
        if kept < n {
            let why = if depth >= self.o.max_depth {
                "depth limit"
            } else {
                "sample"
            };
            self.omit(pointer, format!("array: {kept} of {n} items shown ({why})"));
        }
        Ok(Some(Sample::Array(items)))
    }

    fn object(
        &mut self,
        depth: usize,
        mut shape: Option<&mut Shape>,
        sample: bool,
        pointer: &mut String,
    ) -> Result<Option<Sample<'a>>, JsonViewError> {
        self.i += 1;
        let deep = depth >= self.o.max_shape_depth;
        if let Some(sh) = shape.as_deref_mut() {
            sh.types |= T_OBJECT;
            if deep {
                sh.deeper = true;
            }
        }
        let sample_children = sample && depth < self.o.max_depth;
        let mut fields = Vec::new();
        let mut n = 0usize;
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
        } else {
            loop {
                self.ws();
                if self.b.get(self.i) != Some(&b'"') {
                    return Err(self.err("expected a string key"));
                }
                let kstart = self.i;
                self.string()?;
                let raw_key = &self.s[kstart..self.i];
                self.ws();
                if self.b.get(self.i) != Some(&b':') {
                    return Err(self.err("expected `:` after key"));
                }
                self.i += 1;
                self.ws();
                let key = decode_key(raw_key);
                let take = sample_children && n < self.o.max_keys;
                let len_before = pointer.len();
                pointer.push('/');
                pointer.push_str(&key.replace('~', "~0").replace('/', "~1"));
                let child_shape = match shape.as_deref_mut() {
                    Some(sh) if !deep => shape_key(sh, &key, self.o.max_keys),
                    _ => None,
                };
                let v = self.value(depth + 1, child_shape, take, pointer)?;
                pointer.truncate(len_before);
                if let Some(v) = v {
                    fields.push((raw_key, v));
                }
                n += 1;
                self.ws();
                match self.b.get(self.i) {
                    Some(b',') => self.i += 1,
                    Some(b'}') => {
                        self.i += 1;
                        break;
                    }
                    _ => return Err(self.err("expected `,` or `}` in object")),
                }
            }
        }
        if !sample {
            return Ok(None);
        }
        if fields.len() < n {
            let why = if depth >= self.o.max_depth {
                "depth limit"
            } else {
                "key limit"
            };
            self.omit(
                pointer,
                format!("object: {} of {n} keys shown ({why})", fields.len()),
            );
        }
        Ok(Some(Sample::Object(fields)))
    }
}

fn shape_key<'s>(sh: &'s mut Shape, key: &str, max_keys: usize) -> Option<&'s mut Shape> {
    if let Some(pos) = sh.keys.iter().position(|(k, _)| k == key) {
        return Some(&mut sh.keys[pos].1);
    }
    if sh.keys.len() >= max_keys {
        sh.untracked_keys = true;
        return None;
    }
    sh.keys.push((key.to_string(), Shape::default()));
    sh.keys.last_mut().map(|(_, s)| s)
}

fn decode_key(raw: &str) -> String {
    if raw.contains('\\') {
        serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw[1..raw.len() - 1].to_string())
    } else {
        raw[1..raw.len() - 1].to_string()
    }
}

fn decoded_len(inner: &str) -> usize {
    if inner.contains('\\') {
        serde_json::from_str::<String>(&format!("\"{inner}\""))
            .map(|s| s.chars().count())
            .unwrap_or_else(|_| inner.chars().count())
    } else {
        inner.chars().count()
    }
}

fn type_name(t: u8) -> &'static str {
    match t {
        T_NULL => "null",
        T_BOOL => "boolean",
        T_NUMBER => "number",
        T_STRING => "string",
        T_ARRAY => "array",
        T_OBJECT => "object",
        _ => "mixed",
    }
}

// ─── Rendering ───────────────────────────────────────────────────────────────

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn render(doc: &Doc, bytes: usize, raw: &str, o: &JsonViewOptions, shape_only: bool) -> String {
    let mut out = String::new();
    out.push_str("{\n  \"bricks_view\": \"json-summary/v1\",\n");
    out.push_str(&format!(
        "  \"note\": {},\n",
        json_str(
            "Summary of a JSON document, not the document. `sample` holds verbatim values; \
             `omitted` lists, by JSON Pointer into the original, everything the sample leaves out."
        )
    ));
    out.push_str(&format!(
        "  \"source\": {{ \"bytes\": {bytes}, \"root_type\": {}, \"raw\": {} }},\n",
        json_str(doc.root_type),
        json_str(raw)
    ));
    out.push_str(&format!(
        "  \"limits\": {{ \"sample_items\": {}, \"max_depth\": {}, \"max_keys\": {}, \"max_string_chars\": {} }},\n",
        o.sample_items, o.max_depth, o.max_keys, o.max_string_chars
    ));
    out.push_str("  \"shape\": ");
    render_shape(&doc.shape, 1, &mut out);
    out.push_str(",\n");
    if shape_only {
        out.push_str(
            "  \"sample\": null,\n  \"sample_note\": \"sample dropped to fit the size limit\",\n",
        );
    } else {
        out.push_str("  \"sample\": ");
        match &doc.sample {
            Some(s) => render_sample(s, 1, &mut out),
            None => out.push_str("null"),
        }
        out.push_str(",\n");
    }
    out.push_str("  \"omitted\": [");
    for (k, om) in doc.omitted.iter().enumerate() {
        out.push_str(if k == 0 { "\n" } else { ",\n" });
        out.push_str(&format!(
            "    {{ \"pointer\": {}, \"what\": {} }}",
            json_str(&om.pointer),
            json_str(&om.what)
        ));
    }
    if doc.omitted.len() >= 200 {
        out.push_str(",\n    { \"pointer\": \"\", \"what\": \"further omissions not listed\" }");
    }
    out.push_str(if doc.omitted.is_empty() {
        "]\n}"
    } else {
        "\n  ]\n}"
    });
    out
}

fn pad(depth: usize) -> String {
    "  ".repeat(depth)
}

fn render_shape(sh: &Shape, depth: usize, out: &mut String) {
    let types: Vec<&str> = [
        (T_OBJECT, "object"),
        (T_ARRAY, "array"),
        (T_STRING, "string"),
        (T_NUMBER, "number"),
        (T_BOOL, "boolean"),
        (T_NULL, "null"),
    ]
    .iter()
    .filter(|(b, _)| sh.types & b != 0)
    .map(|(_, n)| *n)
    .collect();
    let mut parts = vec![if types.len() == 1 {
        format!("\"type\": \"{}\"", types[0])
    } else {
        format!(
            "\"type\": [{}]",
            types
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }];
    if sh.types & T_ARRAY != 0 {
        match sh.min_len {
            Some(min) if min != sh.max_len => {
                parts.push(format!("\"length\": [{min}, {}]", sh.max_len))
            }
            _ => parts.push(format!("\"length\": {}", sh.max_len)),
        }
    }
    if sh.deeper {
        parts.push("\"deeper\": \"not described\"".into());
    }
    if sh.untracked_keys {
        parts.push("\"more_keys\": true".into());
    }
    let has_children = !sh.keys.is_empty() || sh.items.as_ref().is_some_and(|i| i.types != 0);
    if !has_children {
        out.push_str(&format!("{{ {} }}", parts.join(", ")));
        return;
    }
    out.push_str("{\n");
    for p in &parts {
        out.push_str(&format!("{}{p},\n", pad(depth + 1)));
    }
    let mut sections = Vec::new();
    if !sh.keys.is_empty() {
        let mut s = format!("{}\"keys\": {{\n", pad(depth + 1));
        for (k, (name, child)) in sh.keys.iter().enumerate() {
            s.push_str(&format!("{}{}: ", pad(depth + 2), json_str(name)));
            render_shape(child, depth + 2, &mut s);
            s.push_str(if k + 1 < sh.keys.len() { ",\n" } else { "\n" });
        }
        s.push_str(&format!("{}}}", pad(depth + 1)));
        sections.push(s);
    }
    if let Some(items) = sh.items.as_ref().filter(|i| i.types != 0) {
        let mut s = format!("{}\"items\": ", pad(depth + 1));
        render_shape(items, depth + 1, &mut s);
        sections.push(s);
    }
    out.push_str(&sections.join(",\n"));
    out.push_str(&format!("\n{}}}", pad(depth)));
}

fn render_sample(s: &Sample, depth: usize, out: &mut String) {
    match s {
        Sample::Raw(r) => out.push_str(r),
        Sample::Cut(c) => out.push_str(c),
        Sample::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (k, it) in items.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                render_sample(it, depth + 1, out);
                out.push_str(if k + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&format!("{}]", pad(depth)));
        }
        Sample::Object(fields) => {
            if fields.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (k, (key, v)) in fields.iter().enumerate() {
                out.push_str(&format!("{}{key}: ", pad(depth + 1)));
                render_sample(v, depth + 1, out);
                out.push_str(if k + 1 < fields.len() { ",\n" } else { "\n" });
            }
            out.push_str(&format!("{}}}", pad(depth)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn big_array(n: usize) -> String {
        let items: Vec<String> = (0..n)
            .map(|i| format!("{{\"id\": {i}, \"price\": 12.50, \"tags\": [\"a\", \"b\"], \"ok\": true, \"note\": null}}"))
            .collect();
        format!("{{\"items\": [{}], \"total\": {n}}}", items.join(", "))
    }

    fn summary(text: &str) -> Value {
        let s = summarize(text, &JsonViewOptions::default(), "Read file_path=/x.json").unwrap();
        serde_json::from_str(&s.text)
            .unwrap_or_else(|e| panic!("summary is not JSON: {e}\n{}", s.text))
    }

    #[test]
    fn large_arrays_keep_three_items_and_report_the_rest() {
        let v = summary(&big_array(5000));
        assert_eq!(v["bricks_view"], "json-summary/v1");
        let items = v["sample"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(v["shape"]["keys"]["items"]["length"], 5000);
        assert_eq!(v["sample"]["total"], 5000);
        let om = v["omitted"].as_array().unwrap();
        assert!(
            om.iter()
                .any(|o| o["pointer"] == "/items"
                    && o["what"].as_str().unwrap().contains("3 of 5000"))
        );
    }

    #[test]
    fn sampled_values_keep_their_exact_form_and_type() {
        let text = r#"{"big": 12345678901234567890123, "price": 12.50, "e": 1E-7, "s": "aé\n", "b": false, "n": null}"#;
        let s = summarize(text, &JsonViewOptions::default(), "raw").unwrap();
        // Digits and escapes exactly as written, not re-serialised through f64.
        assert!(s.text.contains("12345678901234567890123"));
        assert!(s.text.contains("12.50"));
        assert!(s.text.contains("1E-7"));
        assert!(s.text.contains(r#""aé\n""#));
        let v: Value = serde_json::from_str(&s.text).unwrap();
        assert!(v["sample"]["b"].is_boolean());
        assert!(v["sample"]["n"].is_null());
        assert!(v["sample"]["price"].is_number());
    }

    #[test]
    fn keys_that_look_like_the_envelope_cannot_collide() {
        let text = r#"{"bricks_view": "mine", "omitted": [1], "sample": {"x": 1}, "_truncated": true, "list": [1,2,3,4,5]}"#;
        let v = summary(text);
        assert_eq!(v["bricks_view"], "json-summary/v1");
        // The document's own keys are only under `sample`, untouched.
        assert_eq!(v["sample"]["bricks_view"], "mine");
        assert_eq!(v["sample"]["omitted"][0], 1);
        assert_eq!(v["sample"]["sample"]["x"], 1);
        assert_eq!(v["sample"]["_truncated"], true);
        assert_eq!(v["sample"]["list"].as_array().unwrap().len(), 3);
        // Omissions point into the original, never into the sample.
        assert_eq!(v["omitted"][0]["pointer"], "/list");
    }

    #[test]
    fn deep_nesting_is_bounded_and_reported() {
        let mut text = String::new();
        for _ in 0..20 {
            text.push_str("{\"a\": ");
        }
        text.push('1');
        for _ in 0..20 {
            text.push('}');
        }
        let v = summary(&text);
        let om = v["omitted"].as_array().unwrap();
        assert!(
            om.iter()
                .any(|o| o["what"].as_str().unwrap().contains("depth limit")),
            "{v:#}"
        );
        // Too deep for the parser at all: an error, not a crash.
        let deep = "[".repeat(600) + &"]".repeat(600);
        assert!(matches!(
            summarize(&deep, &JsonViewOptions::default(), "raw"),
            Err(JsonViewError::Invalid { .. })
        ));
    }

    #[test]
    fn invalid_json_reports_line_and_column() {
        let text = "{\n  \"a\": 1,\n  \"b\": tru\n}";
        match summarize(text, &JsonViewOptions::default(), "raw") {
            Err(JsonViewError::Invalid { line, column, .. }) => assert_eq!((line, column), (3, 8)),
            other => panic!("{other:?}"),
        }
        assert!(summarize("[1, 2,]", &JsonViewOptions::default(), "raw").is_err());
        assert!(summarize("{\"a\": 1} trailing", &JsonViewOptions::default(), "raw").is_err());
    }

    #[test]
    fn long_strings_are_cut_on_character_boundaries() {
        let long = "é".repeat(500);
        let text = format!(
            "{{\"text\": \"{long}\", \"esc\": \"{}\"}}",
            "\\u00e9".repeat(300)
        );
        let v = summary(&text);
        assert_eq!(v["sample"]["text"].as_str().unwrap().chars().count(), 200);
        assert_eq!(v["sample"]["esc"].as_str().unwrap().chars().count(), 200);
        assert!(v["omitted"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["what"].as_str().unwrap().contains("of 500")));
    }

    #[test]
    fn output_size_is_bounded() {
        let wide: Vec<String> = (0..300)
            .map(|i| format!("\"key_{i}\": \"{}\"", "x".repeat(150)))
            .collect();
        let text = format!("{{{}}}", wide.join(","));
        let opts = JsonViewOptions {
            max_output_bytes: 4000,
            ..Default::default()
        };
        let s = summarize(&text, &opts, "raw").unwrap();
        assert!(s.text.len() <= 4000, "{}", s.text.len());
        assert!(serde_json::from_str::<Value>(&s.text).is_ok());
    }

    #[test]
    fn input_size_is_bounded() {
        let opts = JsonViewOptions {
            max_input_bytes: 10,
            ..Default::default()
        };
        assert!(matches!(
            summarize("[1,2,3,4,5,6]", &opts, "raw"),
            Err(JsonViewError::TooLarge { .. })
        ));
    }

    #[test]
    fn heterogeneous_arrays_report_mixed_types() {
        let v = summary("[1, \"a\", null, {\"k\": 1}, [2]]");
        let t = &v["shape"]["items"]["type"];
        assert!(t.as_array().unwrap().len() >= 4, "{v:#}");
    }
}
