use crate::archives::{add_bytes, add_file, write_zip};
use anyhow::{ensure, Result};
use serde::Deserialize;
use std::{
    fs::{self, File},
    io::{Seek, Write},
    path::Path,
};

#[derive(Deserialize)]
pub struct Chapter {
    pub title: String,
    pub images: Vec<String>,
}
#[derive(Deserialize)]
pub struct Book {
    pub title: String,
    pub author: String,
    pub cover: String,
    pub chapters: Vec<Chapter>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub has_chapters: bool,
}
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn media(path: &str) -> (&'static str, &'static str) {
    match Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" | "jpe" => ("jpg", "image/jpeg"),
        "gif" => ("gif", "image/gif"),
        "webp" => ("webp", "image/webp"),
        _ => ("png", "image/png"),
    }
}
pub fn cbz(book: &Book, output: &Path) -> Result<()> {
    write_zip(output, |zip| {
        let mut metadata = Vec::new();
        let mut page = 1;
        let total: usize = book.chapters.iter().map(|c| c.images.len()).sum();
        let width = total.to_string().len();
        for chapter in &book.chapters {
            let start = page;
            for image in &chapter.images {
                let (ext, _) = media(image);
                add_file(zip, &format!("{page:0width$}.{ext}"), Path::new(image))?;
                page += 1;
            }
            metadata.push(serde_json::json!({"title":chapter.title,"start":start,"end":page-1}));
        }
        let (ext, _) = media(&book.cover);
        add_file(zip, &format!("cover.{ext}"), Path::new(&book.cover))?;
        let metadata = serde_json::json!({"title":book.title,"author":book.author,"tags":book.tags,"chapters":if book.has_chapters {Some(metadata)}else{None}});
        add_bytes(zip, "metadata.json", metadata.to_string().as_bytes(), false)?;
        let info = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><ComicInfo><Title>{}</Title><Series>{}</Series><Writer>{}</Writer><Genre>{}</Genre><PageCount>{total}</PageCount><Manga>Unknown</Manga></ComicInfo>"#,
            xml(&book.title),
            xml(&book.title),
            xml(&book.author),
            xml(&book.tags.join(", "))
        );
        add_bytes(zip, "ComicInfo.xml", info.as_bytes(), false)?;
        Ok(())
    })
}
pub fn epub(book: &Book, output: &Path) -> Result<()> {
    write_zip(output, |zip| {
        add_bytes(zip, "mimetype", b"application/epub+zip", true)?;
        add_bytes(zip,"META-INF/container.xml",br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,false)?;
        let id = uuid::Uuid::new_v4();
        let (ext, mime) = media(&book.cover);
        add_file(zip, &format!("images/cover.{ext}"), Path::new(&book.cover))?;
        let mut manifest = format!(
            r#"<item id="cover" href="images/cover.{ext}" media-type="{mime}" properties="cover-image"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#
        );
        let mut spine = String::new();
        let mut navigation = String::new();
        let mut index = 0;
        for (i, chapter) in book.chapters.iter().enumerate() {
            let mut body = String::new();
            for image in &chapter.images {
                let (ext, mime) = media(image);
                let name = format!("images/{index}.{ext}");
                add_file(zip, &name, Path::new(image))?;
                manifest.push_str(&format!(
                    r#"<item id="img{index}" href="{name}" media-type="{mime}"/>"#
                ));
                body.push_str(&format!(r#"<img src="{name}" alt="{}"/>"#, index + 1));
                index += 1;
            }
            let title = xml(&chapter.title);
            let html = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>{title}</title><style>body{{margin:0}}img{{display:block;max-width:100%;height:auto}}</style></head><body><h1>{title}</h1>{body}</body></html>"#
            );
            add_bytes(zip, &format!("chapter{i}.xhtml"), html.as_bytes(), false)?;
            manifest.push_str(&format!(r#"<item id="chapter{i}" href="chapter{i}.xhtml" media-type="application/xhtml+xml"/>"#));
            spine.push_str(&format!(r#"<itemref idref="chapter{i}"/>"#));
            navigation.push_str(&format!(
                r#"<li><a href="chapter{i}.xhtml">{title}</a></li>"#
            ));
        }
        let title = xml(&book.title);
        let author = xml(&book.author);
        let timestamp = time::OffsetDateTime::now_utc();
        let modified = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            timestamp.year(),
            u8::from(timestamp.month()),
            timestamp.day(),
            timestamp.hour(),
            timestamp.minute(),
            timestamp.second()
        );
        let opf = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><package version="3.0" unique-identifier="book_id" xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title><dc:creator>{author}</dc:creator><dc:language>und</dc:language><dc:identifier id="book_id">urn:uuid:{id}</dc:identifier><meta property="dcterms:modified">{modified}</meta></metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>"#
        );
        add_bytes(zip, "content.opf", opf.as_bytes(), false)?;
        let nav = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>{title}</title></head><body><nav epub:type="toc"><h1>{title}</h1><ol>{navigation}</ol></nav></body></html>"#
        );
        add_bytes(zip, "nav.xhtml", nav.as_bytes(), false)?;
        Ok(())
    })
}

struct Pdf<'a> {
    file: &'a mut File,
    offsets: Vec<u64>,
}
impl Pdf<'_> {
    fn object(&mut self, id: usize, data: &[u8]) -> Result<()> {
        self.offsets[id] = self.file.stream_position()?;
        writeln!(self.file, "{id} 0 obj")?;
        self.file.write_all(data)?;
        writeln!(self.file, "\nendobj")?;
        Ok(())
    }
    fn stream(&mut self, id: usize, attributes: &str, data: &[u8]) -> Result<()> {
        self.offsets[id] = self.file.stream_position()?;
        write!(
            self.file,
            "{id} 0 obj\n<< {attributes} /Length {} >>\nstream\n",
            data.len()
        )?;
        self.file.write_all(data)?;
        write!(self.file, "\nendstream\nendobj\n")?;
        Ok(())
    }
}
fn pdf_string(s: &str) -> String {
    let mut value = String::from("<FEFF");
    for c in s.encode_utf16() {
        value.push_str(&format!("{c:04X}"));
    }
    value.push('>');
    value
}
pub fn pdf(book: &Book, output: &Path) -> Result<()> {
    let pages: Vec<&str> = std::iter::once(book.cover.as_str())
        .chain(
            book.chapters
                .iter()
                .flat_map(|c| c.images.iter().map(String::as_str)),
        )
        .collect();
    ensure!(!pages.is_empty(), "No pages to export");
    let parent = output.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut pdf = Pdf {
        file: temporary.as_file_mut(),
        offsets: vec![0; 4 + pages.len() * 3],
    };
    pdf.file.write_all(b"%PDF-1.7\n%\xFF\xFF\xFF\xFF\n")?;
    pdf.object(1, b"<< /Type /Catalog /Pages 2 0 R >>")?;
    let kids = (0..pages.len())
        .map(|i| format!("{} 0 R", 4 + i * 3))
        .collect::<Vec<_>>()
        .join(" ");
    pdf.object(
        2,
        format!("<< /Type /Pages /Count {} /Kids [{kids}] >>", pages.len()).as_bytes(),
    )?;
    pdf.object(
        3,
        format!(
            "<< /Title {} /Author {} /Producer (Venera Edge) >>",
            pdf_string(&book.title),
            pdf_string(&book.author)
        )
        .as_bytes(),
    )?;
    for (i, path) in pages.iter().enumerate() {
        let image = crate::imaging::decode(&fs::read(path)?)?.to_rgba8();
        let (w, h) = image.dimensions();
        // Composite alpha on white so transparent images render consistently in PDF readers.
        let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
        for p in image.pixels() {
            let a = u16::from(p[3]);
            for c in &p.0[..3] {
                rgb.push(((u16::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8);
            }
        }
        let mut compressor =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        compressor.write_all(&rgb)?;
        let data = compressor.finish()?;
        let id = 4 + i * 3;
        pdf.object(id,format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",id+1,id+2).as_bytes())?;
        pdf.stream(id+1,&format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode"),&data)?;
        pdf.stream(
            id + 2,
            "",
            format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q").as_bytes(),
        )?;
    }
    let xref = pdf.file.stream_position()?;
    write!(
        pdf.file,
        "xref\n0 {}\n0000000000 65535 f \n",
        pdf.offsets.len()
    )?;
    for offset in pdf.offsets.iter().skip(1) {
        writeln!(pdf.file, "{offset:010} 00000 n ")?;
    }
    write!(
        pdf.file,
        "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        pdf.offsets.len()
    )?;
    pdf.file.sync_all()?;
    temporary.persist(output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_unicode_metadata_and_valid_offsets() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let image = dir.path().join("1.png");
        image::RgbaImage::from_pixel(2, 3, image::Rgba([255, 0, 0, 255])).save(&image)?;
        let book = Book {
            title: "中文<&标题".into(),
            author: "作者".into(),
            cover: image.to_string_lossy().into(),
            chapters: vec![Chapter {
                title: "第一章".into(),
                images: vec![image.to_string_lossy().into()],
            }],
            tags: vec![],
            has_chapters: true,
        };
        let epub_path = dir.path().join("book.epub");
        epub(&book, &epub_path)?;
        let mut zip = zip::ZipArchive::new(File::open(epub_path)?)?;
        assert_eq!(zip.by_index(0)?.name(), "mimetype");
        assert_eq!(
            zip.by_index(0)?.compression(),
            zip::CompressionMethod::Stored
        );
        let pdf_path = dir.path().join("book.pdf");
        pdf(&book, &pdf_path)?;
        let bytes = fs::read(pdf_path)?;
        assert!(bytes.starts_with(b"%PDF-1.7"));
        let text = String::from_utf8_lossy(&bytes);
        let offset: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()?;
        assert_eq!(&bytes[offset..offset + 4], b"xref");
        Ok(())
    }
}
