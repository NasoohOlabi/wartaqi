use std::{fmt::Write as _, io::Write as _};

use flate2::{Compression, Crc, write::DeflateEncoder};
use serde_json::Value;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;
const PACKAGE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
const WORKBOOK: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
const WORKBOOK_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;

pub(crate) fn xlsx(rows: &[Vec<Value>], columns: usize) -> Result<Vec<u8>, &'static str> {
    if rows.len() > 1_048_576 || columns > 26 {
        return Err("Course report too large for a workbook");
    }
    let mut worksheet = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cols><col min="1" max="{columns}" width="22" customWidth="1"/></cols><sheetData>"#
    );
    for (index, record) in rows.iter().enumerate() {
        let row = index + 1;
        write!(worksheet, r#"<row r="{row}">"#).unwrap();
        for (index, cell) in record.iter().enumerate() {
            let column = char::from(b'A' + index as u8);
            match cell {
                Value::Number(number) => {
                    write!(worksheet, r#"<c r="{column}{row}"><v>{number}</v></c>"#).unwrap();
                }
                Value::String(text) => {
                    write!(
                        worksheet,
                        r#"<c r="{column}{row}" t="inlineStr"><is><t xml:space="preserve">"#
                    )
                    .unwrap();
                    xml_text(&mut worksheet, text);
                    worksheet.push_str("</t></is></c>");
                }
                _ => {}
            }
        }
        worksheet.push_str("</row>");
    }
    worksheet.push_str("</sheetData></worksheet>");
    zip([
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", PACKAGE_RELS),
        ("xl/workbook.xml", WORKBOOK),
        ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS),
        ("xl/worksheets/sheet1.xml", worksheet.as_str()),
    ])
    .map_err(|_| "Unable to create course workbook")
}

fn xml_text(xml: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => xml.push_str("&amp;"),
            '<' => xml.push_str("&lt;"),
            '>' => xml.push_str("&gt;"),
            '\r' => xml.push_str("&#13;"),
            '\0'..='\u{08}'
            | '\u{0b}'..='\u{0c}'
            | '\u{0e}'..='\u{1f}'
            | '\u{fffe}'
            | '\u{ffff}' => {}
            ch => xml.push(ch),
        }
    }
}

fn zip<const N: usize>(entries: [(&str, &str); N]) -> Result<Vec<u8>, std::io::Error> {
    let mut local = Vec::new();
    let mut central = Vec::new();
    for (name, value) in entries {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(value.as_bytes())?;
        let compressed = encoder.finish()?;
        let mut crc = Crc::new();
        crc.update(value.as_bytes());
        let offset = bounded_len(&local)?;
        let compressed_len = bounded_len(&compressed)?;
        let value_len = bounded_len(value.as_bytes())?;

        local.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        local.extend_from_slice(&[20, 0, 0, 0, 8, 0, 0, 0, 33, 0]);
        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 33, 0]);
        for record in [&mut local, &mut central] {
            record.extend_from_slice(&crc.sum().to_le_bytes());
            record.extend_from_slice(&compressed_len.to_le_bytes());
            record.extend_from_slice(&value_len.to_le_bytes());
            record.extend_from_slice(&(name.len() as u16).to_le_bytes());
            record.extend_from_slice(&[0, 0]);
        }
        local.extend_from_slice(name.as_bytes());
        local.extend_from_slice(&compressed);
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = bounded_len(&local)?;
    let central_len = bounded_len(&central)?;
    local.extend_from_slice(&central);
    local.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    local.extend_from_slice(&[0; 4]);
    local.extend_from_slice(&(N as u16).to_le_bytes());
    local.extend_from_slice(&(N as u16).to_le_bytes());
    local.extend_from_slice(&central_len.to_le_bytes());
    local.extend_from_slice(&central_offset.to_le_bytes());
    local.extend_from_slice(&[0, 0]);
    Ok(local)
}

fn bounded_len(bytes: &[u8]) -> Result<u32, std::io::Error> {
    u32::try_from(bytes.len()).map_err(|_| std::io::Error::other("ZIP member too large"))
}
