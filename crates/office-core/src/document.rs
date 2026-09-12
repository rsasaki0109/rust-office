//! Document tree: Document → Section → Block → (Paragraph | Table | Image).

use serde::{Deserialize, Serialize};

use crate::style::{PageStyle, ParagraphStyle, TextStyle};

/// A contiguous run of text sharing the same character style (and optional link).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    pub style: TextStyle,
    /// Optional hyperlink URL for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

impl Run {
    pub fn new(text: impl Into<String>, style: TextStyle) -> Self {
        Self {
            text: text.into(),
            style,
            link: None,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(text, TextStyle::default())
    }

    pub fn with_link(mut self, link: Option<String>) -> Self {
        self.link = link;
        self
    }

    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn same_formatting(&self, other: &Self) -> bool {
        self.style == other.style && self.link == other.link
    }
}

/// A paragraph composed of styled runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paragraph {
    pub runs: Vec<Run>,
    pub style: ParagraphStyle,
}

impl Paragraph {
    pub fn empty() -> Self {
        Self {
            runs: Vec::new(),
            style: ParagraphStyle::default(),
        }
    }

    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        if text.is_empty() {
            Self::empty()
        } else {
            Self {
                runs: vec![Run::plain(text)],
                style: ParagraphStyle::default(),
            }
        }
    }

    pub fn plain_text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    pub fn char_len(&self) -> usize {
        self.runs.iter().map(Run::char_len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(Run::is_empty)
    }

    /// Normalize empty / adjacent runs so editing stays predictable.
    pub fn normalize(&mut self) {
        self.runs.retain(|r| !r.text.is_empty());
        let mut i = 0;
        while i + 1 < self.runs.len() {
            if self.runs[i].same_formatting(&self.runs[i + 1]) {
                let next = self.runs.remove(i + 1);
                self.runs[i].text.push_str(&next.text);
            } else {
                i += 1;
            }
        }
    }
}

/// One cell in a table (single paragraph for MVP).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableCell {
    pub paragraphs: Vec<Paragraph>,
}

impl TableCell {
    pub fn empty() -> Self {
        Self {
            paragraphs: vec![Paragraph::empty()],
        }
    }

    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            paragraphs: vec![Paragraph::from_text(text)],
        }
    }

    pub fn plain_text(&self) -> String {
        self.paragraphs
            .iter()
            .map(Paragraph::plain_text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A table row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

/// A simple grid table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub rows: Vec<TableRow>,
}

impl Table {
    pub fn new(rows: usize, cols: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Self {
            rows: (0..rows)
                .map(|_| TableRow {
                    cells: (0..cols).map(|_| TableCell::empty()).collect(),
                })
                .collect(),
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn column_count(&self) -> usize {
        self.rows.first().map(|r| r.cells.len()).unwrap_or(0)
    }

    pub fn plain_text(&self) -> String {
        self.rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(TableCell::plain_text)
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Where image bytes / path live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageSource {
    /// Path on the local filesystem (not embedded in the document yet).
    Path { path: String },
    /// Embedded binary (stored as base64 in JSON).
    Embedded {
        mime: String,
        #[serde(with = "serde_bytes_base64")]
        data: Vec<u8>,
    },
}

mod serde_bytes_base64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64_encode(data))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(deserializer)?;
        base64_decode(&s).map_err(serde::de::Error::custom)
    }

    fn base64_encode(data: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let mut buf = [0u8; 3];
            for (i, b) in chunk.iter().enumerate() {
                buf[i] = *b;
            }
            let n = chunk.len();
            let b0 = buf[0] as usize;
            let b1 = buf[1] as usize;
            let b2 = buf[2] as usize;
            out.push(TABLE[b0 >> 2] as char);
            out.push(TABLE[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
            if n > 1 {
                out.push(TABLE[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
            } else {
                out.push('=');
            }
            if n > 2 {
                out.push(TABLE[b2 & 0x3f] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
        fn val(c: u8) -> Result<u8, String> {
            match c {
                b'A'..=b'Z' => Ok(c - b'A'),
                b'a'..=b'z' => Ok(c - b'a' + 26),
                b'0'..=b'9' => Ok(c - b'0' + 52),
                b'+' => Ok(62),
                b'/' => Ok(63),
                _ => Err(format!("invalid base64 char {}", c as char)),
            }
        }
        let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        if bytes.len() % 4 != 0 {
            return Err("invalid base64 length".into());
        }
        let mut out = Vec::new();
        for chunk in bytes.chunks(4) {
            let pad = chunk.iter().filter(|&&c| c == b'=').count();
            let a = val(chunk[0])?;
            let b = val(chunk[1])?;
            let c = if chunk[2] == b'=' { 0 } else { val(chunk[2])? };
            let d = if chunk[3] == b'=' { 0 } else { val(chunk[3])? };
            out.push((a << 2) | (b >> 4));
            if pad < 2 {
                out.push((b << 4) | (c >> 2));
            }
            if pad < 1 {
                out.push((c << 6) | d);
            }
        }
        Ok(out)
    }
}

/// An image anchored in the document flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub source: ImageSource,
    pub alt_text: String,
    /// Display width in points.
    pub width_pt: f32,
    /// Display height in points.
    pub height_pt: f32,
}

impl Image {
    pub fn from_path(path: impl Into<String>, width_pt: f32, height_pt: f32) -> Self {
        let path = path.into();
        let alt_text = std::path::Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".into());
        Self {
            source: ImageSource::Path { path },
            alt_text,
            width_pt,
            height_pt,
        }
    }

    pub fn from_embedded(
        mime: impl Into<String>,
        data: Vec<u8>,
        alt_text: impl Into<String>,
        width_pt: f32,
        height_pt: f32,
    ) -> Self {
        Self {
            source: ImageSource::Embedded {
                mime: mime.into(),
                data,
            },
            alt_text: alt_text.into(),
            width_pt,
            height_pt,
        }
    }

    pub fn display_name(&self) -> &str {
        match &self.source {
            ImageSource::Path { path } => std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(self.alt_text.as_str()),
            ImageSource::Embedded { .. } => self.alt_text.as_str(),
        }
    }

    /// Stable key for texture caching / paint lookup.
    pub fn cache_key(&self) -> String {
        match &self.source {
            ImageSource::Path { path } => format!("path:{path}"),
            ImageSource::Embedded { mime, data } => {
                // Short fingerprint so large embeds aren't keyed by full bytes.
                let mut hash = 0u64;
                for (i, b) in data.iter().take(256).enumerate() {
                    hash = hash.wrapping_mul(31).wrapping_add(*b as u64);
                    hash = hash.wrapping_add(i as u64);
                }
                hash = hash
                    .wrapping_mul(31)
                    .wrapping_add(data.len() as u64);
                format!("embed:{mime}:{hash}")
            }
        }
    }

    pub fn is_embedded(&self) -> bool {
        matches!(self.source, ImageSource::Embedded { .. })
    }
}

/// Guess a MIME type from a file path extension.
pub fn mime_from_path(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "image/png",
    }
}

/// File extension (without dot) for a MIME type.
pub fn extension_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        _ => "png",
    }
}

/// Top-level flow content inside a section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Paragraph(Paragraph),
    Table(Table),
    Image(Image),
    /// Explicit page break — forces the following content onto a new page.
    PageBreak,
}

impl Block {
    pub fn as_paragraph(&self) -> Option<&Paragraph> {
        match self {
            Self::Paragraph(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_paragraph_mut(&mut self) -> Option<&mut Paragraph> {
        match self {
            Self::Paragraph(p) => Some(p),
            _ => None,
        }
    }

    pub fn is_paragraph(&self) -> bool {
        matches!(self, Self::Paragraph(_))
    }
}

/// A document section (future: multiple sections with distinct page styles).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub blocks: Vec<Block>,
    pub page_style: PageStyle,
    /// Optional header paragraph drawn in the top margin of every page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<Paragraph>,
    /// Optional footer paragraph drawn in the bottom margin of every page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<Paragraph>,
}

impl Section {
    pub fn empty() -> Self {
        Self {
            blocks: vec![Block::Paragraph(Paragraph::empty())],
            page_style: PageStyle::default(),
            header: None,
            footer: None,
        }
    }

    /// Ensure at least one paragraph exists for caret placement.
    pub fn ensure_paragraph(&mut self) {
        if !self.blocks.iter().any(Block::is_paragraph) {
            self.blocks.push(Block::Paragraph(Paragraph::empty()));
        }
    }

    pub fn paragraph_count(&self) -> usize {
        self.blocks.iter().filter(|b| b.is_paragraph()).count()
    }

    /// Map a paragraph index (skipping tables/images) to a block index.
    pub fn block_index_of_paragraph(&self, paragraph: usize) -> Option<usize> {
        let mut seen = 0usize;
        for (i, block) in self.blocks.iter().enumerate() {
            if block.is_paragraph() {
                if seen == paragraph {
                    return Some(i);
                }
                seen += 1;
            }
        }
        None
    }

    pub fn paragraph(&self, index: usize) -> Option<&Paragraph> {
        self.block_index_of_paragraph(index)
            .and_then(|i| self.blocks[i].as_paragraph())
    }

    pub fn paragraph_mut(&mut self, index: usize) -> Option<&mut Paragraph> {
        let block_idx = self.block_index_of_paragraph(index)?;
        self.blocks[block_idx].as_paragraph_mut()
    }
}

/// Top-level document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "DocumentWire", into = "DocumentWire")]
pub struct Document {
    pub title: String,
    pub sections: Vec<Section>,
    /// Schema version for the on-disk / interchange format.
    pub format_version: u32,
}

/// Wire format that accepts legacy `paragraphs` arrays.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DocumentWire {
    #[serde(default)]
    title: String,
    #[serde(default)]
    sections: Vec<SectionWire>,
    #[serde(default = "default_format_version")]
    format_version: u32,
}

fn default_format_version() -> u32 {
    Document::CURRENT_FORMAT_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SectionWire {
    #[serde(default)]
    blocks: Option<Vec<Block>>,
    /// Legacy v1 field.
    #[serde(default)]
    paragraphs: Option<Vec<Paragraph>>,
    #[serde(default)]
    page_style: PageStyle,
    #[serde(default)]
    header: Option<Paragraph>,
    #[serde(default)]
    footer: Option<Paragraph>,
}

impl From<DocumentWire> for Document {
    fn from(wire: DocumentWire) -> Self {
        let sections = if wire.sections.is_empty() {
            vec![Section::empty()]
        } else {
            wire.sections
                .into_iter()
                .map(|s| {
                    let blocks = if let Some(blocks) = s.blocks {
                        blocks
                    } else if let Some(paragraphs) = s.paragraphs {
                        paragraphs.into_iter().map(Block::Paragraph).collect()
                    } else {
                        vec![Block::Paragraph(Paragraph::empty())]
                    };
                    let mut section = Section {
                        blocks,
                        page_style: s.page_style,
                        header: s.header,
                        footer: s.footer,
                    };
                    section.ensure_paragraph();
                    section
                })
                .collect()
        };
        Self {
            title: wire.title,
            sections,
            format_version: wire.format_version,
        }
    }
}

impl From<Document> for DocumentWire {
    fn from(doc: Document) -> Self {
        Self {
            title: doc.title,
            sections: doc
                .sections
                .into_iter()
                .map(|s| SectionWire {
                    blocks: Some(s.blocks),
                    paragraphs: None,
                    page_style: s.page_style,
                    header: s.header,
                    footer: s.footer,
                })
                .collect(),
            format_version: doc.format_version,
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub const CURRENT_FORMAT_VERSION: u32 = 3;

    pub fn new() -> Self {
        Self {
            title: String::new(),
            sections: vec![Section::empty()],
            format_version: Self::CURRENT_FORMAT_VERSION,
        }
    }

    pub fn with_text(text: &str) -> Self {
        let mut doc = Self::new();
        let blocks: Vec<Block> = if text.is_empty() {
            vec![Block::Paragraph(Paragraph::empty())]
        } else {
            text.split('\n')
                .map(|line| Block::Paragraph(Paragraph::from_text(line)))
                .collect()
        };
        doc.sections[0].blocks = blocks;
        doc
    }

    pub fn main_section(&self) -> &Section {
        &self.sections[0]
    }

    pub fn main_section_mut(&mut self) -> &mut Section {
        &mut self.sections[0]
    }

    pub fn section_count(&self) -> usize {
        self.sections.len().max(1)
    }

    /// Blocks of section 0 (legacy single-section accessors).
    pub fn blocks(&self) -> &[Block] {
        &self.main_section().blocks
    }

    pub fn blocks_mut(&mut self) -> &mut Vec<Block> {
        &mut self.main_section_mut().blocks
    }

    /// Locate a global paragraph index → `(section_index, local_paragraph_index)`.
    pub fn locate_paragraph(&self, global: usize) -> Option<(usize, usize)> {
        let mut remaining = global;
        for (si, section) in self.sections.iter().enumerate() {
            let n = section.paragraph_count();
            if remaining < n {
                return Some((si, remaining));
            }
            remaining -= n;
        }
        None
    }

    /// Global paragraph index of the first paragraph in `section_index`.
    pub fn paragraph_offset_of_section(&self, section_index: usize) -> usize {
        self.sections
            .iter()
            .take(section_index)
            .map(Section::paragraph_count)
            .sum()
    }

    /// Map global paragraph → `(section_index, block_index)`.
    pub fn block_loc_of_paragraph(&self, global: usize) -> Option<(usize, usize)> {
        let (si, local) = self.locate_paragraph(global)?;
        let bi = self.sections.get(si)?.block_index_of_paragraph(local)?;
        Some((si, bi))
    }

    /// Global paragraph index for a block inside a section.
    pub fn paragraph_index_of_block(&self, section: usize, block_idx: usize) -> usize {
        let before = self.paragraph_offset_of_section(section);
        let local = self
            .sections
            .get(section)
            .map(|s| {
                s.blocks
                    .iter()
                    .take(block_idx)
                    .filter(|b| b.is_paragraph())
                    .count()
            })
            .unwrap_or(0);
        before + local
    }

    /// Number of paragraph blocks across all sections.
    pub fn paragraph_count(&self) -> usize {
        self.sections.iter().map(Section::paragraph_count).sum()
    }

    pub fn paragraph(&self, index: usize) -> Option<&Paragraph> {
        let (si, local) = self.locate_paragraph(index)?;
        self.sections.get(si)?.paragraph(local)
    }

    pub fn paragraph_mut(&mut self, index: usize) -> Option<&mut Paragraph> {
        let (si, local) = self.locate_paragraph(index)?;
        self.sections.get_mut(si)?.paragraph_mut(local)
    }

    /// First paragraph inside a table cell (MVP: one paragraph per cell).
    pub fn cell_paragraph(&self, addr: crate::CellAddress) -> Option<&Paragraph> {
        let Block::Table(table) = self.sections.get(addr.section)?.blocks.get(addr.block)? else {
            return None;
        };
        table
            .rows
            .get(addr.row)?
            .cells
            .get(addr.col)?
            .paragraphs
            .first()
    }

    pub fn cell_paragraph_mut(&mut self, addr: crate::CellAddress) -> Option<&mut Paragraph> {
        let Block::Table(table) = self
            .sections
            .get_mut(addr.section)?
            .blocks
            .get_mut(addr.block)?
        else {
            return None;
        };
        let cell = table.rows.get_mut(addr.row)?.cells.get_mut(addr.col)?;
        if cell.paragraphs.is_empty() {
            cell.paragraphs.push(Paragraph::empty());
        }
        cell.paragraphs.first_mut()
    }

    /// Collect owned clones of all paragraph blocks (for APIs that need a slice).
    pub fn paragraphs_cloned(&self) -> Vec<Paragraph> {
        self.sections
            .iter()
            .flat_map(|s| s.blocks.iter())
            .filter_map(Block::as_paragraph)
            .cloned()
            .collect()
    }

    pub fn page_style(&self) -> &PageStyle {
        &self.main_section().page_style
    }

    pub fn header(&self) -> Option<&Paragraph> {
        self.main_section().header.as_ref()
    }

    pub fn footer(&self) -> Option<&Paragraph> {
        self.main_section().footer.as_ref()
    }

    /// Header for a specific section (falls back to section 0).
    pub fn section_header(&self, section: usize) -> Option<&Paragraph> {
        self.sections
            .get(section)
            .and_then(|s| s.header.as_ref())
            .or_else(|| self.header())
    }

    /// Footer for a specific section (falls back to section 0).
    pub fn section_footer(&self, section: usize) -> Option<&Paragraph> {
        self.sections
            .get(section)
            .and_then(|s| s.footer.as_ref())
            .or_else(|| self.footer())
    }

    pub fn ensure_header_mut(&mut self) -> &mut Paragraph {
        let section = self.main_section_mut();
        if section.header.is_none() {
            section.header = Some(Paragraph::empty());
        }
        section.header.as_mut().unwrap()
    }

    pub fn ensure_footer_mut(&mut self) -> &mut Paragraph {
        let section = self.main_section_mut();
        if section.footer.is_none() {
            section.footer = Some(Paragraph::empty());
        }
        section.footer.as_mut().unwrap()
    }

    /// Hyperlink URL for the character at `pos` (uses previous char when at EOL).
    pub fn link_at(&self, pos: crate::DocPosition) -> Option<&str> {
        let para = self.paragraph(pos.paragraph)?;
        if para.runs.is_empty() {
            return None;
        }
        let total = para.char_len();
        let mut offset = pos.offset;
        if offset >= total && total > 0 {
            offset = total - 1;
        }
        let mut cur = 0usize;
        for run in &para.runs {
            let len = run.char_len();
            if offset < cur + len {
                return run.link.as_deref();
            }
            cur += len;
        }
        para.runs.last().and_then(|r| r.link.as_deref())
    }

    pub fn plain_text(&self) -> String {
        let mut parts = Vec::new();
        for section in &self.sections {
            for block in &section.blocks {
                match block {
                    Block::Paragraph(p) => parts.push(p.plain_text()),
                    Block::Table(t) => parts.push(t.plain_text()),
                    Block::Image(img) => parts.push(format!("[{}]", img.alt_text)),
                    Block::PageBreak => {}
                }
            }
        }
        parts.join("\n")
    }

    pub fn word_count(&self) -> usize {
        self.plain_text()
            .split_whitespace()
            .filter(|w| !w.is_empty())
            .count()
    }

    pub fn char_count(&self) -> usize {
        self.sections
            .iter()
            .flat_map(|s| s.blocks.iter())
            .filter_map(Block::as_paragraph)
            .map(Paragraph::char_len)
            .sum()
    }
}
