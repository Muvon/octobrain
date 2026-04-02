use anyhow::{Context, Result};
use std::io::Cursor;

/// Content type of a source document
#[derive(Debug, Clone, PartialEq)]
pub enum ContentType {
    Html,
    Markdown,
    PlainText,
    Pdf,
    Docx,
}

impl ContentType {
    /// Detect content type from file extension
    pub fn from_extension(path: &str) -> Option<Self> {
        let ext = path.rsplit('.').next()?.to_lowercase();
        match ext.as_str() {
            "html" | "htm" => Some(Self::Html),
            "md" | "markdown" => Some(Self::Markdown),
            "txt" | "text" | "log" | "csv" | "tsv" | "rst" => Some(Self::PlainText),
            "pdf" => Some(Self::Pdf),
            "docx" => Some(Self::Docx),
            _ => None,
        }
    }

    /// Detect content type from HTTP Content-Type header
    pub fn from_content_type_header(header: &str) -> Option<Self> {
        let mime = header.split(';').next()?.trim().to_lowercase();
        match mime.as_str() {
            "text/html" | "application/xhtml+xml" => Some(Self::Html),
            "text/markdown" => Some(Self::Markdown),
            "text/plain" => Some(Self::PlainText),
            "application/pdf" => Some(Self::Pdf),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                Some(Self::Docx)
            }
            _ => None,
        }
    }
}

/// Extract text content from a PDF byte buffer
pub fn extract_text_from_pdf(bytes: &[u8]) -> Result<String> {
    pdf_extract::extract_text_from_mem(bytes).context("Failed to extract text from PDF")
}

/// Extract text content from a DOCX byte buffer
///
/// DOCX is a ZIP archive containing XML files. The main document body
/// is in `word/document.xml`. Text content lives in `<w:t>` elements,
/// paragraphs are delimited by `<w:p>` elements.
pub fn extract_text_from_docx(bytes: &[u8]) -> Result<String> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).context("Failed to open DOCX as ZIP archive")?;

    let doc_xml = archive
        .by_name("word/document.xml")
        .context("DOCX missing word/document.xml")?;

    let mut reader = quick_xml::Reader::from_reader(std::io::BufReader::new(doc_xml));
    let mut output = String::new();
    let mut in_text_node = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(ref e))
            | Ok(quick_xml::events::Event::Empty(ref e)) => {
                let local = e.local_name();
                if local.as_ref() == b"t" {
                    in_text_node = true;
                } else if local.as_ref() == b"p" && !output.is_empty() {
                    // New paragraph
                    output.push('\n');
                } else if local.as_ref() == b"br" || local.as_ref() == b"tab" {
                    output.push(' ');
                }
            }
            Ok(quick_xml::events::Event::End(ref e)) => {
                if e.local_name().as_ref() == b"t" {
                    in_text_node = false;
                }
            }
            Ok(quick_xml::events::Event::Text(ref e)) => {
                if in_text_node {
                    if let Ok(text) = e.decode() {
                        output.push_str(&text);
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => anyhow::bail!("Error parsing DOCX XML: {}", e),
            _ => {}
        }
        buf.clear();
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_type_from_extension() {
        assert_eq!(
            ContentType::from_extension("doc.pdf"),
            Some(ContentType::Pdf)
        );
        assert_eq!(
            ContentType::from_extension("readme.md"),
            Some(ContentType::Markdown)
        );
        assert_eq!(
            ContentType::from_extension("notes.txt"),
            Some(ContentType::PlainText)
        );
        assert_eq!(
            ContentType::from_extension("report.docx"),
            Some(ContentType::Docx)
        );
        assert_eq!(
            ContentType::from_extension("page.html"),
            Some(ContentType::Html)
        );
        assert_eq!(ContentType::from_extension("image.png"), None);
    }

    #[test]
    fn test_content_type_from_header() {
        assert_eq!(
            ContentType::from_content_type_header("text/html; charset=utf-8"),
            Some(ContentType::Html)
        );
        assert_eq!(
            ContentType::from_content_type_header("application/pdf"),
            Some(ContentType::Pdf)
        );
        assert_eq!(
            ContentType::from_content_type_header("text/plain"),
            Some(ContentType::PlainText)
        );
        assert_eq!(
            ContentType::from_content_type_header(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            ),
            Some(ContentType::Docx)
        );
        assert_eq!(ContentType::from_content_type_header("image/png"), None);
    }
}
