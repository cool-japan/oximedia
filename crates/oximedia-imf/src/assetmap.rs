//! Asset Map (ASSETMAP) - SMPTE ST 429-9
//!
//! The ASSETMAP provides a mapping between asset UUIDs and their physical
//! file locations within an IMF package.

use crate::{ImfError, ImfResult};
use chrono::{DateTime, Utc};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Chunk within an asset (for multi-file assets)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    path: PathBuf,
    volume_index: Option<u32>,
    offset: Option<u64>,
    length: Option<u64>,
}

impl Chunk {
    /// Create a new chunk
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            volume_index: None,
            offset: None,
            length: None,
        }
    }

    /// Get the chunk path
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the volume index
    pub fn volume_index(&self) -> Option<u32> {
        self.volume_index
    }

    /// Set the volume index
    pub fn set_volume_index(&mut self, index: u32) {
        self.volume_index = Some(index);
    }

    /// Get the offset
    pub fn offset(&self) -> Option<u64> {
        self.offset
    }

    /// Set the offset
    pub fn set_offset(&mut self, offset: u64) {
        self.offset = Some(offset);
    }

    /// Get the length
    pub fn length(&self) -> Option<u64> {
        self.length
    }

    /// Set the length
    pub fn set_length(&mut self, length: u64) {
        self.length = Some(length);
    }

    /// Builder pattern: with volume index
    pub fn with_volume_index(mut self, index: u32) -> Self {
        self.volume_index = Some(index);
        self
    }

    /// Builder pattern: with offset
    pub fn with_offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Builder pattern: with length
    pub fn with_length(mut self, length: u64) -> Self {
        self.length = Some(length);
        self
    }
}

/// Chunk list for an asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkList {
    chunks: Vec<Chunk>,
}

impl ChunkList {
    /// Create a new chunk list
    pub fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    /// Get the chunks
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    /// Add a chunk
    pub fn add_chunk(&mut self, chunk: Chunk) {
        self.chunks.push(chunk);
    }

    /// Get the total number of chunks
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Check if the chunk list is empty
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Get total length of all chunks (if available)
    pub fn total_length(&self) -> Option<u64> {
        let mut total = 0u64;
        for chunk in &self.chunks {
            total += chunk.length?;
        }
        Some(total)
    }
}

impl Default for ChunkList {
    fn default() -> Self {
        Self::new()
    }
}

/// Asset in an asset map
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    id: Uuid,
    annotation: Option<String>,
    packing_list: bool,
    chunk_list: ChunkList,
}

impl Asset {
    /// Create a new asset
    pub fn new(id: Uuid, packing_list: bool) -> Self {
        Self {
            id,
            annotation: None,
            packing_list,
            chunk_list: ChunkList::new(),
        }
    }

    /// Get the asset ID
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Get the annotation
    pub fn annotation(&self) -> Option<&str> {
        self.annotation.as_deref()
    }

    /// Set annotation
    pub fn set_annotation(&mut self, annotation: String) {
        self.annotation = Some(annotation);
    }

    /// Is this asset a packing list
    pub fn is_packing_list(&self) -> bool {
        self.packing_list
    }

    /// Get the chunk list
    pub fn chunk_list(&self) -> &ChunkList {
        &self.chunk_list
    }

    /// Get mutable chunk list
    pub fn chunk_list_mut(&mut self) -> &mut ChunkList {
        &mut self.chunk_list
    }

    /// Add a chunk to this asset
    pub fn add_chunk(&mut self, chunk: Chunk) {
        self.chunk_list.add_chunk(chunk);
    }

    /// Get the primary file path (first chunk)
    pub fn primary_path(&self) -> Option<&Path> {
        self.chunk_list.chunks.first().map(|c| c.path.as_path())
    }

    /// Builder pattern: with annotation
    pub fn with_annotation(mut self, annotation: String) -> Self {
        self.annotation = Some(annotation);
        self
    }
}

/// Asset Map (ASSETMAP) - SMPTE ST 429-9
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetMap {
    id: Uuid,
    annotation: Option<String>,
    creator: Option<String>,
    volume_count: u32,
    issue_date: DateTime<Utc>,
    issuer: Option<String>,
    assets: Vec<Asset>,
}

impl AssetMap {
    /// Create a new asset map
    pub fn new(id: Uuid) -> Self {
        Self {
            id,
            annotation: None,
            creator: None,
            volume_count: 1,
            issue_date: Utc::now(),
            issuer: None,
            assets: Vec::new(),
        }
    }

    /// Get the asset map ID
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Get the annotation
    pub fn annotation(&self) -> Option<&str> {
        self.annotation.as_deref()
    }

    /// Set annotation
    pub fn set_annotation(&mut self, annotation: String) {
        self.annotation = Some(annotation);
    }

    /// Get the creator
    pub fn creator(&self) -> Option<&str> {
        self.creator.as_deref()
    }

    /// Set creator
    pub fn set_creator(&mut self, creator: String) {
        self.creator = Some(creator);
    }

    /// Get the volume count
    pub fn volume_count(&self) -> u32 {
        self.volume_count
    }

    /// Set volume count
    pub fn set_volume_count(&mut self, count: u32) {
        self.volume_count = count;
    }

    /// Get the issue date
    pub fn issue_date(&self) -> DateTime<Utc> {
        self.issue_date
    }

    /// Set issue date
    pub fn set_issue_date(&mut self, date: DateTime<Utc>) {
        self.issue_date = date;
    }

    /// Get the issuer
    pub fn issuer(&self) -> Option<&str> {
        self.issuer.as_deref()
    }

    /// Set issuer
    pub fn set_issuer(&mut self, issuer: String) {
        self.issuer = Some(issuer);
    }

    /// Get the assets
    pub fn assets(&self) -> &[Asset] {
        &self.assets
    }

    /// Add an asset
    pub fn add_asset(&mut self, asset: Asset) {
        self.assets.push(asset);
    }

    /// Find an asset by ID
    pub fn find_asset(&self, id: Uuid) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// Find an asset by ID (mutable)
    pub fn find_asset_mut(&mut self, id: Uuid) -> Option<&mut Asset> {
        self.assets.iter_mut().find(|a| a.id == id)
    }

    /// Remove an asset by ID
    pub fn remove_asset(&mut self, id: Uuid) -> Option<Asset> {
        if let Some(pos) = self.assets.iter().position(|a| a.id == id) {
            Some(self.assets.remove(pos))
        } else {
            None
        }
    }

    /// Get all packing list assets
    pub fn packing_lists(&self) -> Vec<&Asset> {
        self.assets.iter().filter(|a| a.packing_list).collect()
    }

    /// Get the path for an asset
    pub fn get_asset_path(&self, id: Uuid) -> Option<&Path> {
        self.find_asset(id).and_then(Asset::primary_path)
    }

    /// Parse ASSETMAP from XML
    pub fn from_xml<R: BufRead>(reader: R) -> ImfResult<Self> {
        AssetMapParser::parse(reader)
    }

    /// Write ASSETMAP to XML
    pub fn to_xml<W: Write>(&self, writer: W) -> ImfResult<()> {
        AssetMapWriter::write(self, writer)
    }
}

/// ASSETMAP XML parser
struct AssetMapParser;

impl AssetMapParser {
    #[allow(clippy::too_many_lines)]
    fn parse<R: BufRead>(reader: R) -> ImfResult<AssetMap> {
        let mut xml_reader = Reader::from_reader(reader);
        xml_reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut text_buffer = String::new();

        // AssetMap fields
        let mut id: Option<Uuid> = None;
        let mut annotation: Option<String> = None;
        let mut creator: Option<String> = None;
        let mut volume_count: u32 = 1;
        let mut issue_date: Option<DateTime<Utc>> = None;
        let mut issuer: Option<String> = None;
        let mut assets: Vec<Asset> = Vec::new();

        // State for parsing assets
        let mut in_asset = false;
        let mut current_asset_id: Option<Uuid> = None;
        let mut current_asset_annotation: Option<String> = None;
        let mut current_asset_is_pkl: bool = false;
        let mut current_chunk_list: Vec<Chunk> = Vec::new();

        // State for parsing chunks
        let mut in_chunk = false;
        let mut current_chunk_path: Option<PathBuf> = None;
        let mut current_chunk_volume: Option<u32> = None;
        let mut current_chunk_offset: Option<u64> = None;
        let mut current_chunk_length: Option<u64> = None;

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let element_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    text_buffer.clear();

                    match element_name.as_str() {
                        "Asset" => {
                            in_asset = true;
                            current_asset_id = None;
                            current_asset_annotation = None;
                            current_asset_is_pkl = false;
                            current_chunk_list.clear();
                        }
                        "Chunk" => {
                            in_chunk = true;
                            current_chunk_path = None;
                            current_chunk_volume = None;
                            current_chunk_offset = None;
                            current_chunk_length = None;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(e)) => {
                    text_buffer = String::from_utf8_lossy(e.as_ref()).to_string();
                }
                Ok(Event::End(e)) => {
                    let element_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                    if in_chunk {
                        match element_name.as_str() {
                            "Path" => {
                                current_chunk_path = Some(PathBuf::from(text_buffer.clone()));
                            }
                            "VolumeIndex" => {
                                current_chunk_volume = Some(text_buffer.parse().map_err(|_| {
                                    ImfError::InvalidStructure("Invalid VolumeIndex".to_string())
                                })?);
                            }
                            "Offset" => {
                                current_chunk_offset = Some(text_buffer.parse().map_err(|_| {
                                    ImfError::InvalidStructure("Invalid Offset".to_string())
                                })?);
                            }
                            "Length" => {
                                current_chunk_length = Some(text_buffer.parse().map_err(|_| {
                                    ImfError::InvalidStructure("Invalid Length".to_string())
                                })?);
                            }
                            "Chunk" => {
                                // Build chunk
                                let path = current_chunk_path.take().ok_or_else(|| {
                                    ImfError::MissingElement("Chunk Path".to_string())
                                })?;
                                let mut chunk = Chunk::new(path);
                                if let Some(vol) = current_chunk_volume {
                                    chunk.set_volume_index(vol);
                                }
                                if let Some(offset) = current_chunk_offset {
                                    chunk.set_offset(offset);
                                }
                                if let Some(length) = current_chunk_length {
                                    chunk.set_length(length);
                                }
                                current_chunk_list.push(chunk);
                                in_chunk = false;
                            }
                            _ => {}
                        }
                    } else if in_asset {
                        match element_name.as_str() {
                            "Id" => {
                                current_asset_id = Some(Self::parse_uuid(&text_buffer)?);
                            }
                            "AnnotationText" => {
                                current_asset_annotation = Some(text_buffer.clone());
                            }
                            "PackingList" => {
                                current_asset_is_pkl =
                                    text_buffer.trim().eq_ignore_ascii_case("true");
                            }
                            "Asset" => {
                                // Build asset
                                let asset_id = current_asset_id.ok_or_else(|| {
                                    ImfError::MissingElement("Asset Id".to_string())
                                })?;

                                let mut asset = Asset::new(asset_id, current_asset_is_pkl);
                                asset.annotation = current_asset_annotation.clone();
                                for chunk in &current_chunk_list {
                                    asset.add_chunk(chunk.clone());
                                }
                                assets.push(asset);
                                in_asset = false;
                            }
                            _ => {}
                        }
                    } else {
                        // Top-level elements
                        match element_name.as_str() {
                            "Id" => id = Some(Self::parse_uuid(&text_buffer)?),
                            "AnnotationText" => annotation = Some(text_buffer.clone()),
                            "Creator" => creator = Some(text_buffer.clone()),
                            "VolumeCount" => {
                                volume_count = text_buffer.parse().map_err(|_| {
                                    ImfError::InvalidStructure("Invalid VolumeCount".to_string())
                                })?;
                            }
                            "IssueDate" => {
                                issue_date = Some(
                                    DateTime::parse_from_rfc3339(&text_buffer)
                                        .map_err(|e| {
                                            ImfError::InvalidStructure(format!(
                                                "Invalid IssueDate: {e}"
                                            ))
                                        })?
                                        .with_timezone(&Utc),
                                );
                            }
                            "Issuer" => issuer = Some(text_buffer.clone()),
                            _ => {}
                        }
                    }

                    text_buffer.clear();
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(ImfError::XmlError(format!("XML parse error: {e}"))),
                _ => {}
            }
            buf.clear();
        }

        // Build AssetMap
        let id = id.ok_or_else(|| ImfError::MissingElement("Id".to_string()))?;

        let mut asset_map = AssetMap::new(id);
        asset_map.annotation = annotation;
        asset_map.creator = creator;
        asset_map.volume_count = volume_count;
        asset_map.issue_date = issue_date.unwrap_or_else(Utc::now);
        asset_map.issuer = issuer;
        asset_map.assets = assets;

        Ok(asset_map)
    }

    fn parse_uuid(s: &str) -> ImfResult<Uuid> {
        // Handle URN format: urn:uuid:xxxxx
        let uuid_str = s.trim().strip_prefix("urn:uuid:").unwrap_or(s);
        Uuid::parse_str(uuid_str).map_err(|e| ImfError::InvalidUuid(e.to_string()))
    }
}

/// ASSETMAP XML writer
struct AssetMapWriter;

impl AssetMapWriter {
    fn write<W: Write>(asset_map: &AssetMap, writer: W) -> ImfResult<()> {
        let mut xml_writer = Writer::new_with_indent(writer, b' ', 2);

        // XML declaration
        xml_writer
            .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        // Root element
        let mut root = BytesStart::new("AssetMap");
        root.push_attribute(("xmlns", "http://www.smpte-ra.org/schemas/429-9/2007/AM"));
        xml_writer
            .write_event(Event::Start(root))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        // Write fields
        Self::write_element(&mut xml_writer, "Id", &format!("urn:uuid:{}", asset_map.id))?;

        if let Some(ref annotation) = asset_map.annotation {
            Self::write_element(&mut xml_writer, "AnnotationText", annotation)?;
        }

        if let Some(ref creator) = asset_map.creator {
            Self::write_element(&mut xml_writer, "Creator", creator)?;
        }

        Self::write_element(
            &mut xml_writer,
            "VolumeCount",
            &asset_map.volume_count.to_string(),
        )?;

        Self::write_element(
            &mut xml_writer,
            "IssueDate",
            &asset_map.issue_date.to_rfc3339(),
        )?;

        if let Some(ref issuer) = asset_map.issuer {
            Self::write_element(&mut xml_writer, "Issuer", issuer)?;
        }

        // Assets
        Self::write_assets(&mut xml_writer, &asset_map.assets)?;

        // Close root
        xml_writer
            .write_event(Event::End(BytesEnd::new("AssetMap")))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        Ok(())
    }

    fn write_element<W: Write>(writer: &mut Writer<W>, name: &str, content: &str) -> ImfResult<()> {
        writer
            .write_event(Event::Start(BytesStart::new(name)))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;
        writer
            .write_event(Event::Text(BytesText::new(content)))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;
        writer
            .write_event(Event::End(BytesEnd::new(name)))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;
        Ok(())
    }

    fn write_assets<W: Write>(writer: &mut Writer<W>, assets: &[Asset]) -> ImfResult<()> {
        writer
            .write_event(Event::Start(BytesStart::new("AssetList")))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        for asset in assets {
            Self::write_asset(writer, asset)?;
        }

        writer
            .write_event(Event::End(BytesEnd::new("AssetList")))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        Ok(())
    }

    fn write_asset<W: Write>(writer: &mut Writer<W>, asset: &Asset) -> ImfResult<()> {
        writer
            .write_event(Event::Start(BytesStart::new("Asset")))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        Self::write_element(writer, "Id", &format!("urn:uuid:{}", asset.id))?;

        if let Some(ref annotation) = asset.annotation {
            Self::write_element(writer, "AnnotationText", annotation)?;
        }

        Self::write_element(
            writer,
            "PackingList",
            if asset.packing_list { "true" } else { "false" },
        )?;

        // Chunk list
        Self::write_chunk_list(writer, &asset.chunk_list)?;

        writer
            .write_event(Event::End(BytesEnd::new("Asset")))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        Ok(())
    }

    fn write_chunk_list<W: Write>(writer: &mut Writer<W>, chunk_list: &ChunkList) -> ImfResult<()> {
        writer
            .write_event(Event::Start(BytesStart::new("ChunkList")))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        for chunk in &chunk_list.chunks {
            Self::write_chunk(writer, chunk)?;
        }

        writer
            .write_event(Event::End(BytesEnd::new("ChunkList")))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        Ok(())
    }

    fn write_chunk<W: Write>(writer: &mut Writer<W>, chunk: &Chunk) -> ImfResult<()> {
        writer
            .write_event(Event::Start(BytesStart::new("Chunk")))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        Self::write_element(
            writer,
            "Path",
            chunk
                .path
                .to_str()
                .ok_or_else(|| ImfError::InvalidStructure("Invalid path encoding".to_string()))?,
        )?;

        if let Some(volume_index) = chunk.volume_index {
            Self::write_element(writer, "VolumeIndex", &volume_index.to_string())?;
        }

        if let Some(offset) = chunk.offset {
            Self::write_element(writer, "Offset", &offset.to_string())?;
        }

        if let Some(length) = chunk.length {
            Self::write_element(writer, "Length", &length.to_string())?;
        }

        writer
            .write_event(Event::End(BytesEnd::new("Chunk")))
            .map_err(|e| ImfError::XmlError(format!("Write error: {e}")))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_creation() {
        let chunk = Chunk::new(PathBuf::from("video.mxf"))
            .with_volume_index(1)
            .with_offset(0)
            .with_length(1024);

        assert_eq!(chunk.path(), Path::new("video.mxf"));
        assert_eq!(chunk.volume_index(), Some(1));
        assert_eq!(chunk.offset(), Some(0));
        assert_eq!(chunk.length(), Some(1024));
    }

    #[test]
    fn test_chunk_list() {
        let mut chunk_list = ChunkList::new();

        chunk_list.add_chunk(Chunk::new(PathBuf::from("part1.mxf")).with_length(1000));
        chunk_list.add_chunk(Chunk::new(PathBuf::from("part2.mxf")).with_length(2000));

        assert_eq!(chunk_list.len(), 2);
        assert_eq!(chunk_list.total_length(), Some(3000));
    }

    #[test]
    fn test_asset_creation() {
        let mut asset = Asset::new(Uuid::new_v4(), false);
        asset.set_annotation("Test Asset".to_string());
        asset.add_chunk(Chunk::new(PathBuf::from("test.mxf")));

        assert!(!asset.is_packing_list());
        assert_eq!(asset.annotation(), Some("Test Asset"));
        assert_eq!(asset.chunk_list().len(), 1);
        assert_eq!(asset.primary_path(), Some(Path::new("test.mxf")));
    }

    #[test]
    fn test_asset_map_creation() {
        let mut asset_map = AssetMap::new(Uuid::new_v4());
        asset_map.set_creator("OxiMedia".to_string());
        asset_map.set_issuer("Test Studio".to_string());
        asset_map.set_volume_count(2);

        let asset = Asset::new(Uuid::new_v4(), false);
        let asset_id = asset.id();
        asset_map.add_asset(asset);

        assert_eq!(asset_map.creator(), Some("OxiMedia"));
        assert_eq!(asset_map.volume_count(), 2);
        assert_eq!(asset_map.assets().len(), 1);
        assert!(asset_map.find_asset(asset_id).is_some());
    }

    #[test]
    fn test_packing_list_filter() {
        let mut asset_map = AssetMap::new(Uuid::new_v4());

        asset_map.add_asset(Asset::new(Uuid::new_v4(), true));
        asset_map.add_asset(Asset::new(Uuid::new_v4(), false));
        asset_map.add_asset(Asset::new(Uuid::new_v4(), true));

        assert_eq!(asset_map.packing_lists().len(), 2);
        assert_eq!(asset_map.assets().len(), 3);
    }
}
