//! Parquet access: footer check, codec check and numeric column reads.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parquet::basic::Compression;
use parquet::file::metadata::RowGroupMetaData;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use parquet::schema::types::Type;

use super::{Finding, FindingCode};

/// Directories whose parquet files get a footer check.
const PARQUET_DIRS: [&str; 2] = ["data", "meta"];

/// An open parquet file whose footer has been read.
pub(super) type Reader = SerializedFileReader<File>;

/// A numeric cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Number {
    Int(i64),
    Float(f64),
}

impl Number {
    /// The value as a non-negative integer, `None` for a float or a negative integer.
    pub(super) fn as_u64(self) -> Option<u64> {
        match self {
            Self::Int(value) => u64::try_from(value).ok(),
            Self::Float(_) => None,
        }
    }

    pub(super) fn as_f64(self) -> f64 {
        match self {
            #[expect(
                clippy::cast_precision_loss,
                reason = "timestamps stored as integers are far below 2^52"
            )]
            Self::Int(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

/// Opens `path` and reads its footer. Returns the reason when that fails.
pub(super) fn open(path: &Path) -> Result<Reader, String> {
    let file = File::open(path).map_err(|error| format!("cannot be opened: {error}"))?;
    SerializedFileReader::new(file).map_err(|error| format!("has no readable footer: {error}"))
}

/// Reports every parquet file under `data/` and `meta/` whose footer cannot be read, and
/// returns their paths relative to `root`.
pub(super) fn check_footers(root: &Path, findings: &mut Vec<Finding>) -> BTreeSet<PathBuf> {
    let mut files = Vec::new();
    for dir in PARQUET_DIRS {
        collect_parquet_files(root, Path::new(dir), &mut files, findings);
    }
    files.sort();
    let mut unreadable = BTreeSet::new();
    for path in files {
        if let Err(reason) = open(&root.join(&path)) {
            findings.push(Finding::new(
                FindingCode::ParquetUnreadable,
                &path,
                format!("The file {reason}."),
            ));
            unreadable.insert(path);
        }
    }
    unreadable
}

/// Adds every `*.parquet` file under `root/dir` to `files`, relative to `root`. A
/// directory that cannot be listed is reported.
pub(super) fn collect_parquet_files(
    root: &Path,
    dir: &Path,
    files: &mut Vec<PathBuf>,
    findings: &mut Vec<Finding>,
) {
    let entries = match fs::read_dir(root.join(dir)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            findings.push(Finding::new(
                FindingCode::ParquetUnreadable,
                dir,
                format!("The directory cannot be listed: {error}."),
            ));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                findings.push(Finding::new(
                    FindingCode::ParquetUnreadable,
                    dir,
                    format!("The directory cannot be listed: {error}."),
                ));
                continue;
            }
        };
        let path = dir.join(entry.file_name());
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            collect_parquet_files(root, &path, files, findings);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "parquet")
        {
            files.push(path);
        }
    }
}

/// The first codec of the named columns that is neither uncompressed nor Snappy.
pub(super) fn unsupported_codec(reader: &Reader, names: &[&str]) -> Option<(String, Compression)> {
    reader
        .metadata()
        .row_groups()
        .iter()
        .flat_map(RowGroupMetaData::columns)
        .filter(|column| {
            column
                .column_path()
                .parts()
                .first()
                .is_some_and(|name| names.contains(&name.as_str()))
        })
        .map(|column| (column.column_path().string(), column.compression()))
        .find(|(_, codec)| !matches!(codec, Compression::UNCOMPRESSED | Compression::SNAPPY))
}

/// Reads the named top-level numeric columns, in row order. Returns the reason when a
/// column is missing, uses a codec that is not decoded, or holds a non-numeric value.
pub(super) fn read_columns(reader: &Reader, names: &[&str]) -> Result<Vec<Vec<Number>>, String> {
    if let Some((column, codec)) = unsupported_codec(reader, names) {
        return Err(format!(
            "column {column} uses the codec {codec}, which is not decoded"
        ));
    }
    let schema = reader.metadata().file_metadata().schema();
    let mut fields = Vec::with_capacity(names.len());
    for name in names {
        let field = schema
            .get_fields()
            .iter()
            .find(|field| field.name() == *name)
            .ok_or_else(|| format!("has no column {name}"))?;
        fields.push(Arc::clone(field));
    }
    let projection = Type::group_type_builder(schema.name())
        .with_fields(fields)
        .build()
        .map_err(|error| error.to_string())?;

    let mut columns = vec![Vec::new(); names.len()];
    let rows = reader
        .get_row_iter(Some(projection))
        .map_err(|error| error.to_string())?;
    for row in rows {
        let row = row.map_err(|error| error.to_string())?;
        for (name, field) in row.get_column_iter() {
            let index = names
                .iter()
                .position(|known| known == name)
                .ok_or_else(|| format!("returned the unexpected column {name}"))?;
            let number =
                number(field).ok_or_else(|| format!("column {name} holds the value {field}"))?;
            columns[index].push(number);
        }
    }
    Ok(columns)
}

fn number(field: &Field) -> Option<Number> {
    let number = match *field {
        Field::Byte(value) => Number::Int(value.into()),
        Field::Short(value) => Number::Int(value.into()),
        Field::Int(value) => Number::Int(value.into()),
        Field::Long(value) => Number::Int(value),
        Field::UByte(value) => Number::Int(value.into()),
        Field::UShort(value) => Number::Int(value.into()),
        Field::UInt(value) => Number::Int(value.into()),
        Field::ULong(value) => Number::Int(i64::try_from(value).ok()?),
        Field::Float(value) => Number::Float(value.into()),
        Field::Double(value) => Number::Float(value),
        _ => return None,
    };
    Some(number)
}
