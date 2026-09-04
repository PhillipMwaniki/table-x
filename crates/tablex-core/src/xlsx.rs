//! An Excel workbook, written a row at a time.
//!
//! `.xlsx` is a zip of XML parts, and a zip needs each part's length and
//! checksum in the header that precedes it. That is at odds with streaming: the
//! sheet is the one part whose length is not known until the last row has been
//! seen. So the sheet is streamed to a temporary file while its length and
//! CRC are counted, and the zip is assembled at the end by copying it in —
//! bounded by disk rather than memory, which is the same promise every other
//! export format here makes.
//!
//! The container is written by hand rather than through a compression crate,
//! as stored (uncompressed) entries with ordinary headers, because the
//! alternative formats a streaming writer would need — data descriptors,
//! zip64 — are the ones spreadsheet readers are least reliable about. A stored
//! zip with plain headers opens everywhere; a smaller file that does not open
//! is not a smaller file.
//!
//! Exact numerics are written as text. A spreadsheet cell holds a
//! double-precision float, and putting `NUMERIC(38, 10)` through one is the
//! rounding this application refuses everywhere else. The reader can convert
//! the column; the export does not do it for them.

use crate::value::Value;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;

/// Rows a worksheet can hold, header included. Excel's limit, and the point
/// at which a second sheet is started rather than a truncated one saved.
pub(crate) const SHEET_ROWS: u64 = 1_048_576;

/// A workbook being written: the sheets so far, each in its temporary file.
pub(crate) struct Workbook {
    columns: Vec<String>,
    limit: u64,
    sheets: Vec<Sheet>,
    current: Option<OpenSheet>,
}

/// A finished sheet, waiting to be copied into the zip.
struct Sheet {
    path: PathBuf,
    size: u64,
    crc: u32,
}

/// The sheet rows are going into.
struct OpenSheet {
    path: PathBuf,
    out: Counting<BufWriter<File>>,
    /// Rows written, header included, so the next `r` is known.
    rows: u64,
}

impl Workbook {
    pub(crate) fn start(columns: &[String]) -> io::Result<Self> {
        Self::start_with_limit(columns, SHEET_ROWS)
    }

    /// `limit` is the rows a sheet may hold before the next one is started;
    /// exposed so a test can watch the split without writing a million rows.
    pub(crate) fn start_with_limit(columns: &[String], limit: u64) -> io::Result<Self> {
        let mut workbook = Workbook {
            columns: columns.to_vec(),
            limit: limit.max(2),
            sheets: Vec::new(),
            current: None,
        };
        workbook.open_sheet()?;
        Ok(workbook)
    }

    fn open_sheet(&mut self) -> io::Result<()> {
        let path = std::env::temp_dir().join(format!(
            "tablex-xlsx-{}-{}.xml",
            uuid::Uuid::new_v4(),
            self.sheets.len() + 1
        ));
        let file = File::create(&path)?;
        let mut out = Counting::new(BufWriter::new(file));
        out.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
        )?;
        // The header row, bold, so the sheet reads as a table rather than as
        // rows that happen to start with the column names.
        out.write_all(br#"<row r="1">"#)?;
        for (i, name) in self.columns.iter().enumerate() {
            write!(
                out,
                r#"<c r="{}1" s="1" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
                column_letters(i),
                escape(name)
            )?;
        }
        out.write_all(b"</row>")?;
        self.current = Some(OpenSheet { path, out, rows: 1 });
        Ok(())
    }

    fn close_sheet(&mut self) -> io::Result<()> {
        let Some(mut sheet) = self.current.take() else {
            return Ok(());
        };
        sheet.out.write_all(b"</sheetData></worksheet>")?;
        let (size, crc) = sheet.out.finish()?;
        self.sheets.push(Sheet {
            path: sheet.path,
            size,
            crc,
        });
        Ok(())
    }

    pub(crate) fn row(&mut self, row: &[Value]) -> io::Result<()> {
        if self.current.as_ref().is_some_and(|s| s.rows >= self.limit) {
            self.close_sheet()?;
            self.open_sheet()?;
        }
        let sheet = self
            .current
            .as_mut()
            .ok_or_else(|| io::Error::other("the workbook has been finished"))?;
        sheet.rows += 1;
        let r = sheet.rows;
        write!(sheet.out, r#"<row r="{r}">"#)?;
        for (i, value) in row.iter().enumerate() {
            let reference = format!("{}{r}", column_letters(i));
            match value {
                // An empty cell is the spreadsheet's NULL; writing nothing is
                // the accurate translation.
                Value::Null => {}
                Value::Bool(b) => write!(
                    sheet.out,
                    r#"<c r="{reference}" t="b"><v>{}</v></c>"#,
                    u8::from(*b)
                )?,
                Value::Int(n) => write!(sheet.out, r#"<c r="{reference}"><v>{n}</v></c>"#)?,
                Value::UInt(n) => write!(sheet.out, r#"<c r="{reference}"><v>{n}</v></c>"#)?,
                Value::Float(f) if f.is_finite() => {
                    write!(sheet.out, r#"<c r="{reference}"><v>{f}</v></c>"#)?
                }
                // Everything else — exact numerics included, see the module
                // note — goes in as the text it displays as.
                other => write!(
                    sheet.out,
                    r#"<c r="{reference}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
                    escape(&other.to_string())
                )?,
            }
        }
        sheet.out.write_all(b"</row>")?;
        Ok(())
    }

    /// Assemble the workbook into `sink` and remove the temporary files.
    pub(crate) fn write_to<W: Write>(mut self, sink: &mut W) -> io::Result<()> {
        self.close_sheet()?;
        let mut zip = Zip::new(sink);

        let mut types = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#,
        );
        let mut workbook = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#,
        );
        let mut rels = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#,
        );
        for (i, _) in self.sheets.iter().enumerate() {
            let n = i + 1;
            types.push_str(&format!(
                r#"<Override PartName="/xl/worksheets/sheet{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
            ));
            // The first sheet is just "Rows"; the rest say which part of the
            // result they hold, since the split is Excel's limit, not a
            // division that means anything.
            let name = if n == 1 {
                "Rows".to_string()
            } else {
                format!("Rows {n}")
            };
            workbook.push_str(&format!(
                r#"<sheet name="{name}" sheetId="{n}" r:id="rSheet{n}"/>"#
            ));
            rels.push_str(&format!(
                r#"<Relationship Id="rSheet{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#
            ));
        }
        types.push_str("</Types>");
        workbook.push_str("</sheets></workbook>");
        rels.push_str("</Relationships>");

        zip.add_bytes("[Content_Types].xml", types.as_bytes())?;
        zip.add_bytes(
            "_rels/.rels",
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        )?;
        zip.add_bytes("xl/workbook.xml", workbook.as_bytes())?;
        zip.add_bytes("xl/_rels/workbook.xml.rels", rels.as_bytes())?;
        // Two cell styles: the default, and bold for the header row.
        zip.add_bytes(
            "xl/styles.xml",
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#,
        )?;
        for (i, sheet) in self.sheets.iter().enumerate() {
            let mut file = File::open(&sheet.path)?;
            zip.add_stream(
                &format!("xl/worksheets/sheet{}.xml", i + 1),
                &mut file,
                sheet.size,
                sheet.crc,
            )?;
        }
        zip.finish()?;
        self.cleanup();
        Ok(())
    }

    fn cleanup(&mut self) {
        for sheet in self.sheets.drain(..) {
            let _ = std::fs::remove_file(sheet.path);
        }
        if let Some(sheet) = self.current.take() {
            drop(sheet.out);
            let _ = std::fs::remove_file(sheet.path);
        }
    }
}

impl Drop for Workbook {
    /// An export cancelled or failed halfway must not leave a sheet's worth of
    /// somebody's data in the temp directory.
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// `A`, `B`, … `Z`, `AA`, `AB`, … — spreadsheet column names, zero-based.
fn column_letters(index: usize) -> String {
    let mut n = index + 1;
    let mut out = Vec::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        out.push(b'A' + rem as u8);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii letters")
}

/// Text made safe for an XML text node.
///
/// The five markup characters are escaped; characters XML 1.0 cannot carry at
/// all — control characters other than tab, newline and return — are dropped,
/// because a spreadsheet reader refuses the whole file over one of them.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// A writer that counts what passes through it and keeps a running CRC-32,
/// which is what a zip entry's header needs and what streaming cannot know
/// until the end.
struct Counting<W: Write> {
    inner: W,
    size: u64,
    crc: Crc32,
}

impl<W: Write> Counting<W> {
    fn new(inner: W) -> Self {
        Counting {
            inner,
            size: 0,
            crc: Crc32::new(),
        }
    }

    fn finish(mut self) -> io::Result<(u64, u32)> {
        self.inner.flush()?;
        Ok((self.size, self.crc.value()))
    }
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.size += n as u64;
        self.crc.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// CRC-32 as zip defines it (IEEE 802.3, reflected, `0xEDB88320`).
struct Crc32 {
    state: u32,
}

impl Crc32 {
    fn new() -> Self {
        Crc32 { state: 0xFFFF_FFFF }
    }

    fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            // The table-free form: fold one byte into the low bits, run it
            // through the polynomial, and shift the rest of the state down.
            let mut c = (self.state ^ u32::from(b)) & 0xFF;
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                };
            }
            self.state = (self.state >> 8) ^ c;
        }
    }

    fn value(&self) -> u32 {
        !self.state
    }

    fn of(bytes: &[u8]) -> u32 {
        let mut crc = Crc32::new();
        crc.update(bytes);
        crc.value()
    }
}

/// A zip being written as stored entries, with plain local headers.
struct Zip<'a, W: Write> {
    out: &'a mut W,
    /// Bytes written so far, which is each entry's offset.
    offset: u64,
    entries: Vec<Entry>,
}

struct Entry {
    name: String,
    crc: u32,
    size: u64,
    offset: u64,
}

/// A fixed timestamp: 1980-01-01 00:00, the zip epoch. An export's own
/// modification time is on the file it is saved as.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 0x0021;

impl<'a, W: Write> Zip<'a, W> {
    fn new(out: &'a mut W) -> Self {
        Zip {
            out,
            offset: 0,
            entries: Vec::new(),
        }
    }

    fn header(&mut self, name: &str, size: u64, crc: u32) -> io::Result<()> {
        let size = u32::try_from(size)
            .map_err(|_| io::Error::other("a worksheet is too large for a workbook"))?;
        let mut h = Vec::with_capacity(30 + name.len());
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes()); // version needed
        h.extend_from_slice(&0u16.to_le_bytes()); // flags
        h.extend_from_slice(&0u16.to_le_bytes()); // stored
        h.extend_from_slice(&DOS_TIME.to_le_bytes());
        h.extend_from_slice(&DOS_DATE.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&size.to_le_bytes());
        h.extend_from_slice(&size.to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // extra
        h.extend_from_slice(name.as_bytes());
        self.out.write_all(&h)?;
        self.entries.push(Entry {
            name: name.to_string(),
            crc,
            size: u64::from(size),
            offset: self.offset,
        });
        self.offset += h.len() as u64 + u64::from(size);
        Ok(())
    }

    fn add_bytes(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.header(name, bytes.len() as u64, Crc32::of(bytes))?;
        self.out.write_all(bytes)
    }

    /// An entry whose bytes are copied from a reader, with the size and CRC
    /// counted when they were written.
    fn add_stream<R: Read>(
        &mut self,
        name: &str,
        from: &mut R,
        size: u64,
        crc: u32,
    ) -> io::Result<()> {
        self.header(name, size, crc)?;
        let copied = io::copy(from, self.out)?;
        if copied != size {
            return Err(io::Error::other(
                "a worksheet changed size while being packed",
            ));
        }
        Ok(())
    }

    fn finish(self) -> io::Result<()> {
        let start = self.offset;
        let mut dir = Vec::new();
        for e in &self.entries {
            dir.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            dir.extend_from_slice(&20u16.to_le_bytes()); // made by
            dir.extend_from_slice(&20u16.to_le_bytes()); // needed
            dir.extend_from_slice(&0u16.to_le_bytes());
            dir.extend_from_slice(&0u16.to_le_bytes());
            dir.extend_from_slice(&DOS_TIME.to_le_bytes());
            dir.extend_from_slice(&DOS_DATE.to_le_bytes());
            dir.extend_from_slice(&e.crc.to_le_bytes());
            dir.extend_from_slice(&(e.size as u32).to_le_bytes());
            dir.extend_from_slice(&(e.size as u32).to_le_bytes());
            dir.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&0u16.to_le_bytes()); // extra
            dir.extend_from_slice(&0u16.to_le_bytes()); // comment
            dir.extend_from_slice(&0u16.to_le_bytes()); // disk
            dir.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            dir.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            let offset = u32::try_from(e.offset)
                .map_err(|_| io::Error::other("the workbook is too large for a zip"))?;
            dir.extend_from_slice(&offset.to_le_bytes());
            dir.extend_from_slice(e.name.as_bytes());
        }
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        let start = u32::try_from(start)
            .map_err(|_| io::Error::other("the workbook is too large for a zip"))?;
        end.extend_from_slice(&start.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.out.write_all(&dir)?;
        self.out.write_all(&end)?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    fn workbook(rows: &[Vec<Value>], limit: u64) -> Vec<u8> {
        let columns = vec!["id".to_string(), "name".to_string(), "total".to_string()];
        let mut wb = Workbook::start_with_limit(&columns, limit).unwrap();
        for row in rows {
            wb.row(row).unwrap();
        }
        let mut out = Vec::new();
        wb.write_to(&mut out).unwrap();
        out
    }

    fn contains(haystack: &[u8], needle: &str) -> bool {
        haystack
            .windows(needle.len())
            .any(|w| w == needle.as_bytes())
    }

    #[test]
    fn crc32_matches_the_known_answer() {
        assert_eq!(Crc32::of(b"123456789"), 0xCBF4_3926);
        assert_eq!(Crc32::of(b""), 0);
    }

    #[test]
    fn columns_are_lettered_the_way_a_spreadsheet_letters_them() {
        assert_eq!(column_letters(0), "A");
        assert_eq!(column_letters(25), "Z");
        assert_eq!(column_letters(26), "AA");
        assert_eq!(column_letters(27 * 26 - 1), "ZZ");
        assert_eq!(column_letters(27 * 26), "AAA");
    }

    #[test]
    fn a_workbook_is_a_zip_with_every_part_named() {
        let out = workbook(
            &[vec![
                Value::Int(1),
                text("a"),
                Value::Numeric("1.50".into()),
            ]],
            10,
        );
        assert_eq!(&out[..4], &0x0403_4b50u32.to_le_bytes());
        // The end-of-directory record is the last 22 bytes of a zip with no
        // comment, and starts with its own signature.
        assert_eq!(
            &out[out.len() - 22..out.len() - 18],
            &0x0605_4b50u32.to_le_bytes()
        );
        for part in [
            "[Content_Types].xml",
            "_rels/.rels",
            "xl/workbook.xml",
            "xl/_rels/workbook.xml.rels",
            "xl/styles.xml",
            "xl/worksheets/sheet1.xml",
        ] {
            assert!(contains(&out, part), "missing {part}");
        }
    }

    #[test]
    fn cells_take_the_type_the_value_has() {
        let out = workbook(
            &[vec![Value::Int(7), Value::Bool(true), Value::Float(2.5)]],
            10,
        );
        // Stored entries mean the sheet's XML is in the file verbatim.
        assert!(contains(&out, r#"<c r="A2"><v>7</v></c>"#));
        assert!(contains(&out, r#"<c r="B2" t="b"><v>1</v></c>"#));
        assert!(contains(&out, r#"<c r="C2"><v>2.5</v></c>"#));
        // The header is bold and the sheet is named.
        assert!(contains(
            &out,
            r#"<c r="A1" s="1" t="inlineStr"><is><t xml:space="preserve">id</t></is></c>"#
        ));
        assert!(contains(
            &out,
            r#"<sheet name="Rows" sheetId="1" r:id="rSheet1"/>"#
        ));
    }

    #[test]
    fn exact_numerics_are_text_and_nulls_are_absent() {
        let out = workbook(
            &[vec![
                Value::Numeric("12345678901234567890.25".into()),
                Value::Null,
                text("a < b & \"c\""),
            ]],
            10,
        );
        assert!(contains(
            &out,
            r#"<c r="A2" t="inlineStr"><is><t xml:space="preserve">12345678901234567890.25</t></is></c>"#
        ));
        assert!(!contains(&out, r#"r="B2""#));
        assert!(contains(&out, "a &lt; b &amp; &quot;c&quot;"));
    }

    #[test]
    fn control_characters_are_dropped_rather_than_written() {
        assert_eq!(escape("a\u{1}b\tc"), "a\u{0}b\tc".replace('\u{0}', ""));
    }

    #[test]
    fn a_sheet_that_fills_starts_the_next() {
        // Three rows per sheet, header included: two data rows each.
        let rows: Vec<Vec<Value>> = (1..=5)
            .map(|i| vec![Value::Int(i), text("x"), Value::Null])
            .collect();
        let out = workbook(&rows, 3);
        assert!(contains(&out, "xl/worksheets/sheet3.xml"));
        assert!(!contains(&out, "xl/worksheets/sheet4.xml"));
        assert!(contains(
            &out,
            r#"<sheet name="Rows 3" sheetId="3" r:id="rSheet3"/>"#
        ));
        // The fifth row is the first data row of the third sheet, numbered 2.
        assert!(contains(&out, r#"<row r="2"><c r="A2"><v>5</v></c>"#));
    }
}
