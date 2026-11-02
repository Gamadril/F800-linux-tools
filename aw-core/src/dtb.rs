//! Flattened device tree (DTB) <-> device tree source (DTS) conversion.
//!
//! A small dependency free implementation that is sufficient to round-trip the DTBs stored in
//! Allwinner boot packages and to compile hand edited sources back into a DTB.
//!
//! Supported DTS syntax: `/dts-v1/`, `/memreserve/`, nodes with labels, properties holding
//! strings, `<cells>`, `/bits/ 8|16|32|64 <...>`, `[hex bytes]`, `&label` / `&{/path}`
//! references (phandles are allocated when a referenced node has none), `/delete-property/`,
//! `/delete-node/`, repeated `/ { }` blocks, `&label { }` blocks, C style integer expressions in
//! parentheses (`<(1 << 5)>`), value labels and comments.
//!
//! `&label { }` blocks must come after the node that defines the label.
//!
//! Not supported: `/include/`, `/incbin/`, `/omit-if-no-ref/`, the C preprocessor and `/plugin/`
//! overlays.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;
const HEADER_LEN: usize = 40;
const MAX_DEPTH: usize = 64;
const MAX_RESERVE: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
    /// Node name including the unit address; empty for the root node.
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    pub boot_cpuid_phys: u32,
    pub reserve: Vec<(u64, u64)>,
    pub root: Node,
}

/// True when `data` starts with the flattened device tree magic.
pub fn is_dtb(data: &[u8]) -> bool {
    data.len() >= 4 && data[..4] == FDT_MAGIC.to_be_bytes()
}

// ---------------------------------------------------------------------------------------------
// DTB -> tree
// ---------------------------------------------------------------------------------------------

fn be32(b: &[u8], o: usize) -> Result<u32> {
    o.checked_add(4)
        .and_then(|e| b.get(o..e))
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
        .ok_or_else(|| anyhow!("device tree truncated at offset {o:#x}"))
}

fn be64(b: &[u8], o: usize) -> Result<u64> {
    o.checked_add(8)
        .and_then(|e| b.get(o..e))
        .map(|s| u64::from_be_bytes(s.try_into().unwrap()))
        .ok_or_else(|| anyhow!("device tree truncated at offset {o:#x}"))
}

fn block<'a>(b: &'a [u8], off: usize, len: usize, what: &str) -> Result<&'a [u8]> {
    off.checked_add(len)
        .and_then(|e| b.get(off..e))
        .ok_or_else(|| anyhow!("device tree {what} is out of range"))
}

fn str_at(b: &[u8], o: usize, what: &str) -> Result<String> {
    let rest = b
        .get(o..)
        .ok_or_else(|| anyhow!("device tree {what} offset {o:#x} is out of range"))?;
    let end = rest
        .iter()
        .position(|&c| c == 0)
        .ok_or_else(|| anyhow!("device tree {what} at {o:#x} is not NUL terminated"))?;
    String::from_utf8(rest[..end].to_vec())
        .map_err(|_| anyhow!("device tree {what} at {o:#x} is not valid UTF-8"))
}

pub fn parse_dtb(data: &[u8]) -> Result<Tree> {
    if !is_dtb(data) {
        bail!("not a flattened device tree (bad magic)");
    }
    let total = be32(data, 4)? as usize;
    let off_struct = be32(data, 8)? as usize;
    let off_strings = be32(data, 12)? as usize;
    let off_rsv = be32(data, 16)? as usize;
    let version = be32(data, 20)?;
    let last_comp = be32(data, 24)?;
    let boot_cpuid_phys = be32(data, 28)?;
    let size_strings = be32(data, 32)? as usize;
    if version < 16 || last_comp > 17 {
        bail!("unsupported device tree version {version} (compatible with {last_comp})");
    }
    let data = data
        .get(..total)
        .ok_or_else(|| anyhow!("device tree totalsize {total:#x} exceeds the available data"))?;
    let size_struct = if version >= 17 {
        be32(data, 36)? as usize
    } else {
        total.saturating_sub(off_struct)
    };
    let strings = block(data, off_strings, size_strings, "strings block")?;
    let st = block(data, off_struct, size_struct, "structure block")?;

    let mut reserve = Vec::new();
    let mut o = off_rsv;
    loop {
        let (addr, size) = (be64(data, o)?, be64(data, o + 8)?);
        o += 16;
        if addr == 0 && size == 0 {
            break;
        }
        reserve.push((addr, size));
        if reserve.len() > MAX_RESERVE {
            bail!("device tree has too many memory reservations");
        }
    }

    let mut pos = 0usize;
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    loop {
        let token = be32(st, pos)?;
        pos += 4;
        match token {
            FDT_BEGIN_NODE => {
                if stack.is_empty() && root.is_some() {
                    bail!("device tree has more than one root node");
                }
                if stack.len() >= MAX_DEPTH {
                    bail!("device tree nodes are nested too deeply");
                }
                let name = str_at(st, pos, "node name")?;
                pos = (pos + name.len() + 1 + 3) & !3;
                stack.push(Node {
                    name,
                    ..Node::default()
                });
            }
            FDT_END_NODE => {
                let node = stack
                    .pop()
                    .ok_or_else(|| anyhow!("device tree has an unbalanced end-of-node token"))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            FDT_PROP => {
                let len = be32(st, pos)? as usize;
                let name_off = be32(st, pos + 4)? as usize;
                pos += 8;
                let value = block(st, pos, len, "property value")?.to_vec();
                pos = (pos + len + 3) & !3;
                let name = str_at(strings, name_off, "property name")?;
                stack
                    .last_mut()
                    .ok_or_else(|| anyhow!("device tree property outside of a node"))?
                    .props
                    .push(Prop { name, value });
            }
            FDT_NOP => {}
            FDT_END => break,
            t => bail!("unknown device tree token {t:#x}"),
        }
    }
    if !stack.is_empty() {
        bail!("device tree has an unterminated node");
    }
    Ok(Tree {
        boot_cpuid_phys,
        reserve,
        root: root.ok_or_else(|| anyhow!("device tree has no root node"))?,
    })
}

// ---------------------------------------------------------------------------------------------
// tree -> DTS
// ---------------------------------------------------------------------------------------------

fn is_string_list(v: &[u8]) -> bool {
    if v.last() != Some(&0) {
        return false;
    }
    v[..v.len() - 1].split(|&b| b == 0).all(|s| {
        !s.is_empty()
            && s.iter()
                .all(|&b| (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\t')
    })
}

fn push_quoted(out: &mut String, bytes: &[u8]) {
    out.push('"');
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            _ => out.push(b as char),
        }
    }
    out.push('"');
}

fn push_value(out: &mut String, v: &[u8]) {
    if is_string_list(v) {
        for (i, s) in v[..v.len() - 1].split(|&b| b == 0).enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            push_quoted(out, s);
        }
    } else if v.len().is_multiple_of(4) {
        out.push('<');
        for (i, c) in v.as_chunks::<4>().0.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            let _ = write!(out, "{:#x}", u32::from_be_bytes(*c));
        }
        out.push('>');
    } else {
        out.push('[');
        for (i, b) in v.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            let _ = write!(out, "{b:02x}");
        }
        out.push(']');
    }
}

fn push_node(out: &mut String, node: &Node, depth: usize) {
    let indent = "\t".repeat(depth);
    let name = if node.name.is_empty() {
        "/"
    } else {
        &node.name
    };
    let _ = writeln!(out, "{indent}{name} {{");
    for p in &node.props {
        let _ = write!(out, "{indent}\t{}", p.name);
        if !p.value.is_empty() {
            out.push_str(" = ");
            push_value(out, &p.value);
        }
        out.push_str(";\n");
    }
    for c in &node.children {
        out.push('\n');
        push_node(out, c, depth + 1);
    }
    let _ = writeln!(out, "{indent}}};");
}

pub fn tree_to_dts(tree: &Tree) -> String {
    let mut out = String::from("/dts-v1/;\n\n");
    for (addr, size) in &tree.reserve {
        let _ = writeln!(out, "/memreserve/ {addr:#x} {size:#x};");
    }
    if !tree.reserve.is_empty() {
        out.push('\n');
    }
    push_node(&mut out, &tree.root, 0);
    out
}

/// Decompile a DTB into readable DTS text.
///
/// The text is re-parsed and compared against the original tree, so a DTS is only returned when
/// compiling it again yields an equivalent device tree.
pub fn dtb_to_dts(dtb: &[u8]) -> Result<String> {
    let tree = parse_dtb(dtb)?;
    let text = tree_to_dts(&tree);
    let mut back = parse_dts(&text).context("generated DTS does not parse back")?;
    back.boot_cpuid_phys = tree.boot_cpuid_phys;
    if back != tree {
        bail!("device tree cannot be represented losslessly as DTS");
    }
    Ok(text)
}

// ---------------------------------------------------------------------------------------------
// DTS -> tree
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
enum Target {
    Label(String),
    Path(String),
}

enum Seg {
    Bytes(Vec<u8>),
    Phandle(Target),
    PathRef(Target),
}

struct PProp {
    name: String,
    segs: Vec<Seg>,
}

struct PNode {
    name: String,
    labels: Vec<String>,
    props: Vec<PProp>,
    children: Vec<PNode>,
}

impl PNode {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            labels: Vec::new(),
            props: Vec::new(),
            children: Vec::new(),
        }
    }
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b",._+*#?@-".contains(&c)
}

fn node_by_label<'a>(n: &'a mut PNode, label: &str) -> Option<&'a mut PNode> {
    if n.labels.iter().any(|l| l == label) {
        return Some(n);
    }
    n.children.iter_mut().find_map(|c| node_by_label(c, label))
}

fn node_by_path_mut<'a>(root: &'a mut PNode, path: &str) -> Option<&'a mut PNode> {
    if !path.starts_with('/') {
        return None;
    }
    let mut cur = root;
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        cur = cur.children.iter_mut().find(|c| c.name == comp)?;
    }
    Some(cur)
}

fn node_by_path<'a>(root: &'a PNode, path: &str) -> Option<&'a PNode> {
    if !path.starts_with('/') {
        return None;
    }
    let mut cur = root;
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        cur = cur.children.iter().find(|c| c.name == comp)?;
    }
    Some(cur)
}

fn remove_by_label(n: &mut PNode, label: &str) -> bool {
    if let Some(i) = n
        .children
        .iter()
        .position(|c| c.labels.iter().any(|l| l == label))
    {
        n.children.remove(i);
        return true;
    }
    n.children.iter_mut().any(|c| remove_by_label(c, label))
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
    /// Nesting of expression operands, to bound recursion on hostile input.
    depth: usize,
}

impl Parser<'_> {
    fn fail<T>(&self, msg: impl std::fmt::Display) -> Result<T> {
        let before = &self.s[..self.pos.min(self.s.len())];
        let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
        let col = before.iter().rev().take_while(|&&b| b != b'\n').count() + 1;
        bail!("DTS syntax error at line {line}, column {col}: {msg}")
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn lit(&mut self, l: &str) -> bool {
        if self.s[self.pos..].starts_with(l.as_bytes()) {
            self.pos += l.len();
            true
        } else {
            false
        }
    }

    fn skip(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => self.pos += 1,
                Some(b'/') if self.s.get(self.pos + 1) == Some(&b'/') => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.s.get(self.pos + 1) == Some(&b'*') => {
                    let start = self.pos;
                    self.pos += 2;
                    loop {
                        match self.s.get(self.pos) {
                            None => {
                                self.pos = start;
                                return self.fail("unterminated comment");
                            }
                            Some(b'*') if self.s.get(self.pos + 1) == Some(&b'/') => {
                                self.pos += 2;
                                break;
                            }
                            _ => self.pos += 1,
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn eat(&mut self, c: u8) -> Result<bool> {
        self.skip()?;
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect(&mut self, c: u8) -> Result<()> {
        if self.eat(c)? {
            Ok(())
        } else {
            self.fail(format!("expected '{}'", c as char))
        }
    }

    fn name(&mut self) -> Result<String> {
        let start = self.pos;
        while self.peek().is_some_and(is_name_char) {
            self.pos += 1;
        }
        if start == self.pos {
            return self.fail("expected a node or property name");
        }
        Ok(String::from_utf8_lossy(&self.s[start..self.pos]).into_owned())
    }

    fn reference(&mut self) -> Result<Target> {
        self.pos += 1; // '&'
        if self.peek() == Some(b'{') {
            self.pos += 1;
            let start = self.pos;
            while self.peek().is_some_and(|c| c != b'}') {
                self.pos += 1;
            }
            if self.peek() != Some(b'}') {
                return self.fail("unterminated '&{' reference");
            }
            let path = String::from_utf8_lossy(&self.s[start..self.pos]).into_owned();
            self.pos += 1;
            return Ok(Target::Path(path));
        }
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.pos += 1;
        }
        if start == self.pos {
            return self.fail("expected a label after '&'");
        }
        Ok(Target::Label(
            String::from_utf8_lossy(&self.s[start..self.pos]).into_owned(),
        ))
    }

    fn escape(&mut self) -> Result<u8> {
        let Some(c) = self.peek() else {
            return self.fail("unterminated escape sequence");
        };
        self.pos += 1;
        Ok(match c {
            b'n' => b'\n',
            b't' => b'\t',
            b'r' => b'\r',
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'v' => 11,
            b'0'..=b'7' => {
                let mut v = u32::from(c - b'0');
                for _ in 0..2 {
                    match self.peek() {
                        Some(d @ b'0'..=b'7') => {
                            v = v * 8 + u32::from(d - b'0');
                            self.pos += 1;
                        }
                        _ => break,
                    }
                }
                if v > 0xff {
                    return self.fail("octal escape out of range");
                }
                v as u8
            }
            b'x' => {
                let mut v = 0u32;
                let mut n = 0;
                while n < 2 {
                    match self.peek().and_then(|d| (d as char).to_digit(16)) {
                        Some(d) => {
                            v = v * 16 + d;
                            self.pos += 1;
                            n += 1;
                        }
                        None => break,
                    }
                }
                if n == 0 {
                    return self.fail("expected hex digits after '\\x'");
                }
                v as u8
            }
            other => other,
        })
    }

    /// A string literal including its terminating NUL.
    fn string(&mut self) -> Result<Vec<u8>> {
        self.pos += 1; // opening quote
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None => return self.fail("unterminated string"),
                Some(b'"') => {
                    self.pos += 1;
                    out.push(0);
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    out.push(self.escape()?);
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Result<u64> {
        if self.peek() == Some(b'\'') {
            self.pos += 1;
            let c = match self.peek() {
                Some(b'\\') => {
                    self.pos += 1;
                    self.escape()?
                }
                Some(c) if c != b'\'' => {
                    self.pos += 1;
                    c
                }
                _ => return self.fail("invalid character literal"),
            };
            if self.peek() != Some(b'\'') {
                return self.fail("unterminated character literal");
            }
            self.pos += 1;
            return Ok(u64::from(c));
        }
        let start = self.pos;
        let (radix, digits_start) =
            if self.s[self.pos..].starts_with(b"0x") || self.s[self.pos..].starts_with(b"0X") {
                (16, self.pos + 2)
            } else if self.peek() == Some(b'0') {
                (8, self.pos)
            } else {
                (10, self.pos)
            };
        self.pos = digits_start;
        while self
            .peek()
            .is_some_and(|c| (c as char).is_digit(if radix == 8 { 10 } else { radix }))
        {
            self.pos += 1;
        }
        let digits = String::from_utf8_lossy(&self.s[digits_start..self.pos]).into_owned();
        while matches!(self.peek(), Some(b'u' | b'U' | b'l' | b'L')) {
            self.pos += 1;
        }
        if digits.is_empty() && radix != 8 {
            self.pos = start;
            return self.fail("expected a number");
        }
        let digits = if digits.is_empty() { "0" } else { &digits };
        match u64::from_str_radix(digits, radix) {
            Ok(v) => Ok(v),
            Err(_) => {
                self.pos = start;
                self.fail("invalid or out-of-range number")
            }
        }
    }

    /// Consume `label:` if present; the label of a value is not needed for the binary.
    fn try_label(&mut self) -> Result<Option<String>> {
        self.skip()?;
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.pos += 1;
        }
        if self.pos > start {
            let end = self.pos;
            self.skip()?;
            if self.peek() == Some(b':') {
                self.pos += 1;
                return Ok(Some(
                    String::from_utf8_lossy(&self.s[start..end]).into_owned(),
                ));
            }
        }
        self.pos = start;
        Ok(None)
    }

    fn operator(&self) -> Option<(&'static str, u8)> {
        // Two character operators first so that "<<" is not read as "<".
        const OPS: [(&str, u8); 18] = [
            ("||", 1),
            ("&&", 2),
            ("==", 6),
            ("!=", 6),
            ("<=", 7),
            (">=", 7),
            ("<<", 8),
            (">>", 8),
            ("|", 3),
            ("^", 4),
            ("&", 5),
            ("<", 7),
            (">", 7),
            ("+", 9),
            ("-", 9),
            ("*", 10),
            ("/", 10),
            ("%", 10),
        ];
        OPS.into_iter()
            .find(|(op, _)| self.s[self.pos..].starts_with(op.as_bytes()))
    }

    fn apply(&self, op: &str, a: u64, b: u64) -> Result<u64> {
        Ok(match op {
            "||" => u64::from(a != 0 || b != 0),
            "&&" => u64::from(a != 0 && b != 0),
            "|" => a | b,
            "^" => a ^ b,
            "&" => a & b,
            "==" => u64::from(a == b),
            "!=" => u64::from(a != b),
            "<=" => u64::from(a <= b),
            ">=" => u64::from(a >= b),
            "<" => u64::from(a < b),
            ">" => u64::from(a > b),
            "<<" | ">>" => {
                if b >= 64 {
                    return self.fail("shift count out of range");
                }
                if op == "<<" {
                    a << b
                } else {
                    a >> b
                }
            }
            "+" => a.wrapping_add(b),
            "-" => a.wrapping_sub(b),
            "*" => a.wrapping_mul(b),
            "/" | "%" => {
                if b == 0 {
                    return self.fail("division by zero");
                }
                if op == "/" {
                    a / b
                } else {
                    a % b
                }
            }
            _ => return self.fail("unknown operator"),
        })
    }

    fn binary(&mut self, min_prec: u8) -> Result<u64> {
        let mut lhs = self.unary()?;
        loop {
            self.skip()?;
            let Some((op, prec)) = self.operator() else {
                return Ok(lhs);
            };
            if prec < min_prec {
                return Ok(lhs);
            }
            self.pos += op.len();
            let rhs = self.binary(prec + 1)?;
            lhs = self.apply(op, lhs, rhs)?;
        }
    }

    fn expr(&mut self) -> Result<u64> {
        let cond = self.binary(1)?;
        if self.eat(b'?')? {
            let yes = self.expr()?;
            self.expect(b':')?;
            let no = self.expr()?;
            return Ok(if cond != 0 { yes } else { no });
        }
        Ok(cond)
    }

    fn unary(&mut self) -> Result<u64> {
        if self.depth >= 128 {
            return self.fail("expression nested too deeply");
        }
        self.depth += 1;
        let result = self.unary_inner();
        self.depth -= 1;
        result
    }

    fn unary_inner(&mut self) -> Result<u64> {
        self.skip()?;
        match self.peek() {
            Some(b'-') => {
                self.pos += 1;
                Ok(self.unary()?.wrapping_neg())
            }
            Some(b'~') => {
                self.pos += 1;
                Ok(!self.unary()?)
            }
            Some(b'!') => {
                self.pos += 1;
                Ok(u64::from(self.unary()? == 0))
            }
            Some(b'(') => {
                self.pos += 1;
                let v = self.expr()?;
                self.expect(b')')?;
                Ok(v)
            }
            _ => self.number(),
        }
    }

    /// Append one cell of `bits` width. Like dtc, negative values (all high bits set) are accepted.
    fn push_cell(&self, bits: u32, v: u64, buf: &mut Vec<u8>) -> Result<()> {
        if bits < 64 {
            let mask = (1u64 << bits) - 1;
            if v & !mask != 0 && (v | mask) != u64::MAX {
                return self.fail(format!("value {v:#x} does not fit in {bits} bits"));
            }
        }
        buf.extend_from_slice(&v.to_be_bytes()[(8 - bits / 8) as usize..]);
        Ok(())
    }

    fn cells(&mut self, bits: u32, segs: &mut Vec<Seg>) -> Result<()> {
        let mut buf = Vec::new();
        loop {
            while self.try_label()?.is_some() {}
            self.skip()?;
            match self.peek() {
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(b'&') => {
                    if bits != 32 {
                        return self.fail("references are only allowed in 32-bit cells");
                    }
                    if !buf.is_empty() {
                        segs.push(Seg::Bytes(std::mem::take(&mut buf)));
                    }
                    segs.push(Seg::Phandle(self.reference()?));
                }
                Some(b'(') => {
                    let v = self.unary()?;
                    self.push_cell(bits, v, &mut buf)?;
                }
                Some(_) => {
                    let v = self.number()?;
                    self.push_cell(bits, v, &mut buf)?;
                }
                None => return self.fail("unterminated '<'"),
            }
        }
        segs.push(Seg::Bytes(buf));
        Ok(())
    }

    fn bytes(&mut self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            self.skip()?;
            match self.peek() {
                Some(b']') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(c) if c.is_ascii_hexdigit() => {
                    let hi = (c as char).to_digit(16).unwrap();
                    let Some(lo) = self
                        .s
                        .get(self.pos + 1)
                        .and_then(|&d| (d as char).to_digit(16))
                    else {
                        return self.fail("byte strings need two hex digits per byte");
                    };
                    out.push((hi * 16 + lo) as u8);
                    self.pos += 2;
                }
                _ => return self.fail("expected hex bytes or ']'"),
            }
        }
    }

    fn values(&mut self) -> Result<Vec<Seg>> {
        let mut segs = Vec::new();
        loop {
            while self.try_label()?.is_some() {}
            self.skip()?;
            match self.peek() {
                Some(b'"') => segs.push(Seg::Bytes(self.string()?)),
                Some(b'<') => {
                    self.pos += 1;
                    self.cells(32, &mut segs)?;
                }
                Some(b'[') => {
                    self.pos += 1;
                    segs.push(Seg::Bytes(self.bytes()?));
                }
                Some(b'&') => segs.push(Seg::PathRef(self.reference()?)),
                Some(b'/') if self.lit("/bits/") => {
                    self.skip()?;
                    let bits = self.number()?;
                    if !matches!(bits, 8 | 16 | 32 | 64) {
                        return self.fail("/bits/ must be 8, 16, 32 or 64");
                    }
                    self.expect(b'<')?;
                    self.cells(bits as u32, &mut segs)?;
                }
                _ => return self.fail("expected a property value"),
            }
            while self.try_label()?.is_some() {}
            if !self.eat(b',')? {
                return Ok(segs);
            }
        }
    }

    fn body(&mut self, node: &mut PNode, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return self.fail("nodes are nested too deeply");
        }
        loop {
            self.skip()?;
            if matches!(self.peek(), None | Some(b'}')) {
                return Ok(());
            }
            if self.lit("/delete-property/") {
                self.skip()?;
                let name = self.name()?;
                self.expect(b';')?;
                node.props.retain(|p| p.name != name);
                continue;
            }
            if self.lit("/delete-node/") {
                self.skip()?;
                if self.peek() == Some(b'&') {
                    return self.fail("use '/delete-node/ &label;' at the top level of the file");
                }
                let name = self.name()?;
                self.expect(b';')?;
                node.children.retain(|c| c.name != name);
                continue;
            }
            if self.peek() == Some(b'/') {
                let end = self.s[self.pos..]
                    .iter()
                    .take(32)
                    .position(|c| c.is_ascii_whitespace())
                    .unwrap_or(32.min(self.s.len() - self.pos));
                let word = String::from_utf8_lossy(&self.s[self.pos..self.pos + end]).into_owned();
                return self.fail(format!("unsupported directive '{word}'"));
            }
            let mut labels = Vec::new();
            let name = loop {
                let word = self.name()?;
                self.skip()?;
                if self.peek() == Some(b':') {
                    self.pos += 1;
                    labels.push(word);
                    self.skip()?;
                } else {
                    break word;
                }
            };
            if self.peek() == Some(b'{') {
                self.pos += 1;
                let idx = match node.children.iter().position(|c| c.name == name) {
                    Some(i) => i,
                    None => {
                        node.children.push(PNode::new(&name));
                        node.children.len() - 1
                    }
                };
                let child = &mut node.children[idx];
                for l in labels {
                    if !child.labels.contains(&l) {
                        child.labels.push(l);
                    }
                }
                self.body(child, depth + 1)?;
                self.expect(b'}')?;
                self.expect(b';')?;
            } else {
                let segs = if self.eat(b'=')? {
                    self.values()?
                } else {
                    Vec::new()
                };
                self.expect(b';')?;
                let prop = PProp { name, segs };
                match node.props.iter_mut().find(|p| p.name == prop.name) {
                    Some(existing) => *existing = prop,
                    None => node.props.push(prop),
                }
            }
        }
    }

    fn file(&mut self) -> Result<(PNode, Vec<(u64, u64)>)> {
        self.skip()?;
        if !self.lit("/dts-v1/") {
            return self.fail("the file must start with '/dts-v1/;'");
        }
        self.expect(b';')?;
        let mut root = PNode::new("");
        let mut have_root = false;
        let mut reserve = Vec::new();
        loop {
            self.skip()?;
            if self.peek().is_none() {
                break;
            }
            let mut top_labels = Vec::new();
            while let Some(l) = self.try_label()? {
                top_labels.push(l);
            }
            if self.lit("/memreserve/") {
                self.skip()?;
                let addr = self.number()?;
                self.skip()?;
                let size = self.number()?;
                self.expect(b';')?;
                reserve.push((addr, size));
            } else if self.lit("/delete-node/") {
                self.skip()?;
                if self.peek() != Some(b'&') {
                    return self.fail("expected '&label' after '/delete-node/'");
                }
                let target = self.reference()?;
                self.expect(b';')?;
                let removed = match &target {
                    Target::Label(l) => remove_by_label(&mut root, l),
                    Target::Path(p) => match p.rsplit_once('/') {
                        Some((parent, leaf)) => {
                            let parent = if parent.is_empty() { "/" } else { parent };
                            node_by_path_mut(&mut root, parent).is_some_and(|n| {
                                let before = n.children.len();
                                n.children.retain(|c| c.name != leaf);
                                n.children.len() != before
                            })
                        }
                        None => false,
                    },
                };
                if !removed {
                    return self.fail("/delete-node/ target does not exist");
                }
            } else if self.peek() == Some(b'/') {
                self.pos += 1;
                self.expect(b'{')?;
                have_root = true;
                for l in top_labels {
                    if !root.labels.contains(&l) {
                        root.labels.push(l);
                    }
                }
                self.body(&mut root, 0)?;
                self.expect(b'}')?;
                self.expect(b';')?;
            } else if self.peek() == Some(b'&') {
                let target = self.reference()?;
                self.expect(b'{')?;
                let node = match &target {
                    Target::Label(l) => node_by_label(&mut root, l),
                    Target::Path(p) => node_by_path_mut(&mut root, p),
                };
                let Some(node) = node else {
                    return self
                        .fail("reference to a node that is not defined earlier in the file");
                };
                self.body(node, 1)?;
                self.expect(b'}')?;
                self.expect(b';')?;
            } else {
                return self.fail(
                    "expected '/ {', '&label {', '/memreserve/' or '/delete-node/' \
                     (preprocessor directives and /include/ are not supported)",
                );
            }
        }
        if !have_root {
            bail!("DTS has no root node");
        }
        Ok((root, reserve))
    }
}

fn collect_labels(n: &PNode, path: &str, map: &mut HashMap<String, String>) -> Result<()> {
    for l in &n.labels {
        if map.insert(l.clone(), path.to_string()).is_some() {
            bail!("duplicate label '{l}'");
        }
    }
    for c in &n.children {
        let p = if path == "/" {
            format!("/{}", c.name)
        } else {
            format!("{path}/{}", c.name)
        };
        collect_labels(c, &p, map)?;
    }
    Ok(())
}

fn static_value(p: &PProp) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for s in &p.segs {
        match s {
            Seg::Bytes(b) => out.extend_from_slice(b),
            _ => return None,
        }
    }
    Some(out)
}

fn existing_phandle(n: &PNode) -> Option<u32> {
    n.props
        .iter()
        .filter(|p| p.name == "phandle" || p.name == "linux,phandle")
        .find_map(|p| {
            static_value(p)
                .filter(|v| v.len() == 4)
                .map(|v| u32::from_be_bytes(v.try_into().unwrap()))
        })
}

fn collect_phandles(n: &PNode, out: &mut HashSet<u32>) {
    out.extend(existing_phandle(n));
    for c in &n.children {
        collect_phandles(c, out);
    }
}

fn has_phandle_prop(n: &PNode) -> bool {
    n.props
        .iter()
        .any(|p| p.name == "phandle" || p.name == "linux,phandle")
}

fn collect_phandle_refs(n: &PNode, out: &mut Vec<Target>) {
    for p in &n.props {
        for s in &p.segs {
            if let Seg::Phandle(t) = s {
                out.push(t.clone());
            }
        }
    }
    for c in &n.children {
        collect_phandle_refs(c, out);
    }
}

fn target_path(labels: &HashMap<String, String>, t: &Target) -> Result<String> {
    match t {
        Target::Label(l) => labels
            .get(l)
            .cloned()
            .ok_or_else(|| anyhow!("reference to undefined label '&{l}'")),
        Target::Path(p) => Ok(p.clone()),
    }
}

fn finish(
    n: &PNode,
    root: &PNode,
    labels: &HashMap<String, String>,
    handles: &HashMap<String, u32>,
) -> Result<Node> {
    let mut props = Vec::new();
    for p in &n.props {
        let mut value = Vec::new();
        for s in &p.segs {
            match s {
                Seg::Bytes(b) => value.extend_from_slice(b),
                Seg::Phandle(t) => {
                    let path = target_path(labels, t)?;
                    value.extend_from_slice(&handles[&path].to_be_bytes());
                }
                Seg::PathRef(t) => {
                    let path = target_path(labels, t)?;
                    if node_by_path(root, &path).is_none() {
                        bail!("reference to unknown node '{path}'");
                    }
                    value.extend_from_slice(path.as_bytes());
                    value.push(0);
                }
            }
        }
        props.push(Prop {
            name: p.name.clone(),
            value,
        });
    }
    let children = n
        .children
        .iter()
        .map(|c| finish(c, root, labels, handles))
        .collect::<Result<_>>()?;
    Ok(Node {
        name: n.name.clone(),
        props,
        children,
    })
}

pub fn parse_dts(text: &str) -> Result<Tree> {
    let text = text.trim_start_matches('\u{feff}');
    let mut parser = Parser {
        s: text.as_bytes(),
        pos: 0,
        depth: 0,
    };
    let (mut root, reserve) = parser.file()?;

    let mut labels = HashMap::new();
    collect_labels(&root, "/", &mut labels)?;

    // Referenced nodes without a phandle get the first unused value (from 1), like dtc does.
    let mut refs = Vec::new();
    collect_phandle_refs(&root, &mut refs);
    let mut handles: HashMap<String, u32> = HashMap::new();
    let mut assigned = Vec::new();
    let mut used = HashSet::new();
    collect_phandles(&root, &mut used);
    let mut next = 1u32;
    for t in refs {
        let path = target_path(&labels, &t)?;
        if handles.contains_key(&path) {
            continue;
        }
        let node = node_by_path(&root, &path)
            .ok_or_else(|| anyhow!("reference to unknown node '{path}'"))?;
        let handle = match existing_phandle(node) {
            Some(h) => h,
            None => {
                while used.contains(&next) {
                    next = next
                        .checked_add(1)
                        .ok_or_else(|| anyhow!("out of phandle values"))?;
                }
                used.insert(next);
                assigned.push((path.clone(), next));
                next
            }
        };
        handles.insert(path, handle);
    }
    for (path, handle) in assigned {
        // `phandle = <&self>` already carries the property; it resolves to the new value.
        if let Some(node) = node_by_path_mut(&mut root, &path).filter(|n| !has_phandle_prop(n)) {
            node.props.push(PProp {
                name: "phandle".into(),
                segs: vec![Seg::Bytes(handle.to_be_bytes().to_vec())],
            });
        }
    }

    Ok(Tree {
        boot_cpuid_phys: 0,
        reserve,
        root: finish(&root, &root, &labels, &handles)?,
    })
}

// ---------------------------------------------------------------------------------------------
// tree -> DTB
// ---------------------------------------------------------------------------------------------

fn string_offset(table: &mut Vec<u8>, cache: &mut HashMap<String, u32>, name: &str) -> Result<u32> {
    if let Some(&off) = cache.get(name) {
        return Ok(off);
    }
    let needle = name.as_bytes();
    // Like dtc, share the tail of an earlier name when possible.
    let found = (0..table.len())
        .find(|&i| table[i..].starts_with(needle) && table.get(i + needle.len()) == Some(&0));
    let off = match found {
        Some(i) => i,
        None => {
            let i = table.len();
            table.extend_from_slice(needle);
            table.push(0);
            i
        }
    };
    let off = u32::try_from(off)?;
    cache.insert(name.to_string(), off);
    Ok(off)
}

fn pad4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

fn write_node(
    st: &mut Vec<u8>,
    strings: &mut Vec<u8>,
    cache: &mut HashMap<String, u32>,
    node: &Node,
    depth: usize,
) -> Result<()> {
    if depth > MAX_DEPTH {
        bail!("device tree nodes are nested too deeply");
    }
    if node.name.contains('\0') {
        bail!("node name {:?} contains a NUL byte", node.name);
    }
    st.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
    st.extend_from_slice(node.name.as_bytes());
    st.push(0);
    pad4(st);
    for p in &node.props {
        if p.name.contains('\0') {
            bail!("property name {:?} contains a NUL byte", p.name);
        }
        let off = string_offset(strings, cache, &p.name)?;
        let len = u32::try_from(p.value.len())
            .map_err(|_| anyhow!("property '{}' is too large", p.name))?;
        st.extend_from_slice(&FDT_PROP.to_be_bytes());
        st.extend_from_slice(&len.to_be_bytes());
        st.extend_from_slice(&off.to_be_bytes());
        st.extend_from_slice(&p.value);
        pad4(st);
    }
    for c in &node.children {
        write_node(st, strings, cache, c, depth + 1)?;
    }
    st.extend_from_slice(&FDT_END_NODE.to_be_bytes());
    Ok(())
}

/// Serialize a tree as a version 17 DTB (same block order as `dtc`).
pub fn tree_to_dtb(tree: &Tree) -> Result<Vec<u8>> {
    tree_to_dtb_padded(tree, 0)
}

/// Like [`tree_to_dtb`], with `pad` zero bytes counted in `totalsize` after the strings block
/// (what `dtc -p` produces; Allwinner images reserve room for U-Boot to grow the tree).
pub fn tree_to_dtb_padded(tree: &Tree, pad: usize) -> Result<Vec<u8>> {
    let mut st = Vec::new();
    let mut strings = Vec::new();
    let mut cache = HashMap::new();
    write_node(&mut st, &mut strings, &mut cache, &tree.root, 0)?;
    st.extend_from_slice(&FDT_END.to_be_bytes());

    let off_rsv = HEADER_LEN;
    let off_struct = off_rsv + (tree.reserve.len() + 1) * 16;
    let off_strings = off_struct + st.len();
    let total = off_strings + strings.len() + pad;
    let to32 = |v: usize| u32::try_from(v).map_err(|_| anyhow!("device tree is larger than 4 GiB"));

    let mut out = Vec::with_capacity(total);
    for v in [
        FDT_MAGIC,
        to32(total)?,
        to32(off_struct)?,
        to32(off_strings)?,
        to32(off_rsv)?,
        17,
        16,
        tree.boot_cpuid_phys,
        to32(strings.len())?,
        to32(st.len())?,
    ] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    for (addr, size) in &tree.reserve {
        out.extend_from_slice(&addr.to_be_bytes());
        out.extend_from_slice(&size.to_be_bytes());
    }
    out.extend_from_slice(&[0; 16]);
    out.extend_from_slice(&st);
    out.extend_from_slice(&strings);
    out.resize(total, 0);
    Ok(out)
}

/// Header field that DTS cannot express; taken from the DTB that is being replaced.
fn template_boot_cpuid(template: Option<&[u8]>) -> u32 {
    template
        .filter(|t| is_dtb(t))
        .and_then(|t| be32(t, 28).ok())
        .unwrap_or(0)
}

/// Padding after the strings block of the DTB being replaced, so a rebuilt tree keeps it.
fn template_padding(template: Option<&[u8]>) -> usize {
    let Some(t) = template.filter(|t| is_dtb(t)) else {
        return 0;
    };
    let field = |o| be32(t, o).ok().map(|v| v as usize);
    match (field(4), field(12), field(32)) {
        (Some(total), Some(off), Some(len)) if total <= t.len() => {
            total.saturating_sub(off.saturating_add(len))
        }
        _ => 0,
    }
}

/// Compile DTS text into a DTB. `template` is the DTB being replaced (if any).
pub fn dts_to_dtb(dts: &str, template: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut tree = parse_dts(dts)?;
    tree.boot_cpuid_phys = template_boot_cpuid(template);
    tree_to_dtb_padded(&tree, template_padding(template))
}

/// Compile `dts` only if it describes a different device tree than `original`.
///
/// Comment, whitespace and formatting edits return `None`, so the original binary is kept.
pub fn compile_if_changed(dts: &str, original: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut tree = parse_dts(dts)?;
    tree.boot_cpuid_phys = template_boot_cpuid(Some(original));
    if parse_dtb(original).is_ok_and(|old| old == tree) {
        return Ok(None);
    }
    tree_to_dtb_padded(&tree, template_padding(Some(original))).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
/dts-v1/;
/memreserve/ 0x40000000 0x100000;
/ {
	#address-cells = <1>;
	#size-cells = <1>;
	model = "Test Board";
	compatible = "vendor,board", "vendor,soc";
	chosen { bootargs = "console=ttyS0,115200 root=/dev/mtdblock4"; };
	intc: interrupt-controller@3021000 {
		compatible = "arm,gic";
		interrupt-controller;
		reg = <0x3021000 0x1000>;
		#interrupt-cells = <3>;
	};
	soc {
		serial@2500000 {
			interrupt-parent = <&intc>;
			interrupts = <0 2 4>;
			mac = [00 11 22 33 44 55];
			odd = /bits/ 16 <0x1234 0x5678>;
			wide = /bits/ 64 <0x1122334455667788>;
			ref-path = &intc;
			status = "okay";
		};
	};
};
"#;

    fn prop<'a>(n: &'a Node, name: &str) -> &'a [u8] {
        &n.props.iter().find(|p| p.name == name).unwrap().value
    }

    #[test]
    fn matches_dtc_layout_for_empty_tree() {
        let dtb = dts_to_dtb("/dts-v1/;\n/ {\n};\n", None).unwrap();
        let words: Vec<u32> = dtb
            .chunks_exact(4)
            .map(|c| u32::from_be_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(
            words,
            [
                0xd00dfeed, 0x48, 0x38, 0x48, 0x28, 17, 16, 0, 0, 0x10, // header
                0, 0, 0, 0, // empty reservation map
                1, 0, 2, 9, // BEGIN_NODE "", END_NODE, END
            ]
        );
    }

    #[test]
    fn references_and_value_types() {
        let tree = parse_dts(SAMPLE).unwrap();
        assert_eq!(tree.reserve, [(0x4000_0000, 0x10_0000)]);
        let root = &tree.root;
        assert_eq!(prop(root, "model"), b"Test Board\0");
        assert_eq!(prop(root, "compatible"), b"vendor,board\0vendor,soc\0");
        let intc = &root.children[1];
        assert_eq!(prop(intc, "phandle"), 1u32.to_be_bytes());
        let serial = &root.children[2].children[0];
        assert_eq!(prop(serial, "interrupt-parent"), 1u32.to_be_bytes());
        assert_eq!(prop(serial, "mac"), [0, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(prop(serial, "odd"), [0x12, 0x34, 0x56, 0x78]);
        assert_eq!(prop(serial, "wide"), 0x1122334455667788u64.to_be_bytes());
        assert_eq!(prop(serial, "ref-path"), b"/interrupt-controller@3021000\0");
    }

    #[test]
    fn dtb_dts_roundtrip_is_stable() {
        let dtb = dts_to_dtb(SAMPLE, None).unwrap();
        let dts = dtb_to_dts(&dtb).unwrap();
        assert!(dts.contains("compatible = \"vendor,board\", \"vendor,soc\";"));
        assert!(dts.contains("mac = [00 11 22 33 44 55];"));
        assert!(dts.contains("/memreserve/ 0x40000000 0x100000;"));
        assert_eq!(dts_to_dtb(&dts, None).unwrap(), dtb);
        assert_eq!(dtb_to_dts(&dtb).unwrap(), dts);
    }

    #[test]
    fn merging_overlays_and_deletes() {
        let tree = parse_dts(
            "/dts-v1/;
/ { a = <1>; b = <2>; n1 { x; }; l: n2 { y = <1>; }; };
/ { a = <3>; /delete-property/ b; /delete-node/ n1; };
&l { y = <9>; z; };
",
        )
        .unwrap();
        let root = &tree.root;
        assert_eq!(root.props.len(), 1);
        assert_eq!(prop(root, "a"), 3u32.to_be_bytes());
        assert_eq!(root.children.len(), 1);
        assert_eq!(prop(&root.children[0], "y"), 9u32.to_be_bytes());
        assert!(prop(&root.children[0], "z").is_empty());
    }

    #[test]
    fn syntax_errors_report_position() {
        let e = parse_dts("/dts-v1/;\n/ {\n\tfoo = <1>\n};\n").unwrap_err();
        assert!(e.to_string().contains("line 4"), "{e}");
        assert!(parse_dts("/dts-v1/;\n/ { a = <0x100000000>; };").is_err());
        assert!(parse_dts("/dts-v1/;\n/ { a = <&nowhere>; };").is_err());
        assert!(parse_dts("#include <x.h>\n/dts-v1/;\n/ {};").is_err());
        assert!(parse_dts("/ { };").is_err());
        assert!(parse_dts("/dts-v1/;\n/ { a = \"x; };").is_err());
    }

    #[test]
    fn unrepresentable_trees_are_rejected_not_corrupted() {
        let mut tree = parse_dts(SAMPLE).unwrap();
        tree.root.props.push(Prop {
            name: "bad name".into(),
            value: vec![1],
        });
        let dtb = tree_to_dtb(&tree).unwrap();
        assert!(dtb_to_dts(&dtb).is_err());
    }

    #[test]
    fn malformed_dtbs_do_not_panic() {
        let dtb = dts_to_dtb(SAMPLE, None).unwrap();
        for cut in 0..dtb.len() {
            let _ = parse_dtb(&dtb[..cut]);
        }
        for i in 4..dtb.len() {
            let mut bad = dtb.clone();
            bad[i] ^= 0xff;
            let _ = parse_dtb(&bad);
        }
        assert!(parse_dtb(b"not a dtb at all, definitely").is_err());
    }

    #[test]
    fn change_detection_ignores_formatting_and_keeps_boot_cpuid() {
        let mut original = dts_to_dtb(SAMPLE, None).unwrap();
        original[28..32].copy_from_slice(&7u32.to_be_bytes());
        let text = dtb_to_dts(&original).unwrap();

        let reformatted = format!(
            "// edited\n{}\n\n/* trailing */\n",
            text.replace('\t', "    ")
        );
        assert!(compile_if_changed(&reformatted, &original)
            .unwrap()
            .is_none());

        let edited = text.replace("Test Board", "Other Board");
        let new = compile_if_changed(&edited, &original).unwrap().unwrap();
        assert_ne!(new, original);
        assert_eq!(&new[28..32], &7u32.to_be_bytes());
        let tree = parse_dtb(&new).unwrap();
        assert_eq!(prop(&tree.root, "model"), b"Other Board\0");
    }

    #[test]
    fn expressions_value_labels_and_sign_extension() {
        let tree = parse_dts(
            "/dts-v1/;\nl: / { a = <(1 << 5 | 3) (-1) (2 + 3 * 4) (7 > 3 ? 10 : 20)>;\n\
             b = start: <0x1 mid: 0x2> end:, \"s\"; c = /bits/ 8 <(-2) 'A'>; };\n",
        )
        .unwrap();
        assert_eq!(
            prop(&tree.root, "a"),
            [0, 0, 0, 0x23, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 14, 0, 0, 0, 10]
        );
        assert_eq!(prop(&tree.root, "b"), b"\0\0\0\x01\0\0\0\x02s\0");
        assert_eq!(prop(&tree.root, "c"), [0xfe, b'A']);
        assert!(parse_dts("/dts-v1/;\n/ { a = <(1 / 0)>; };").is_err());
        assert!(parse_dts("/dts-v1/;\n/ { a = <(1 << 64)>; };").is_err());
        assert!(parse_dts(&format!(
            "/dts-v1/;\n/ {{ a = <{}1{}>; }};",
            "(".repeat(500),
            ")".repeat(500)
        ))
        .is_err());
    }

    #[test]
    fn phandles_follow_dtc_allocation() {
        let tree = parse_dts(
            "/dts-v1/;\n/ { a: a { }; b: b { phandle = <&b>; }; c: c { phandle = <1>; };\n\
             d { x = <&a &b &c>; }; };\n",
        )
        .unwrap();
        let p = |n: &str| {
            prop(
                tree.root.children.iter().find(|c| c.name == n).unwrap(),
                "phandle",
            )
            .to_vec()
        };
        // 1 is taken explicitly; implicit values go out in tree order (b's self reference comes first)
        assert_eq!(p("a"), 3u32.to_be_bytes());
        assert_eq!(p("b"), 2u32.to_be_bytes());
        assert_eq!(p("c"), 1u32.to_be_bytes());
        let b = &tree.root.children[1];
        assert_eq!(b.props.iter().filter(|p| p.name == "phandle").count(), 1);
    }

    #[test]
    fn padding_of_the_replaced_dtb_is_preserved() {
        let mut original = dts_to_dtb(SAMPLE, None).unwrap();
        let total = original.len() + 2048;
        original.resize(total, 0);
        original[4..8].copy_from_slice(&(total as u32).to_be_bytes());
        let edited = dtb_to_dts(&original)
            .unwrap()
            .replace("Test Board", "Other");
        let new = compile_if_changed(&edited, &original).unwrap().unwrap();
        let unpadded = dts_to_dtb(&edited, None).unwrap();
        assert_eq!(new.len(), unpadded.len() + 2048);
        assert_eq!(
            u32::from_be_bytes(new[4..8].try_into().unwrap()) as usize,
            new.len()
        );
        assert_eq!(parse_dtb(&new).unwrap(), parse_dtb(&unpadded).unwrap());
    }
}
