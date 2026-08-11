use std::fs::{self, File};
use std::path::{Path, PathBuf};

use larian_formats::lsf::LsfData;
use quick_xml::events::Event;
use quick_xml::reader::NsReader;
use tempfile::Builder;
use thiserror::Error;

/// Inputs for one native LSF/LSX conversion.
#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    /// Existing `.lsf` or `.lsx` source file.
    #[arg(value_name = "SOURCE")]
    source: PathBuf,
    /// New `.lsx` or `.lsf` destination file.
    #[arg(value_name = "DESTINATION")]
    destination: PathBuf,
    /// Replaces an existing destination after conversion succeeds.
    #[arg(long)]
    force: bool,
}

/// Failures specific to native LSF/LSX conversion.
#[derive(Debug, Error)]
pub enum Error {
    #[error(
        "unsupported conversion from `{input}` to `{output}`; expected one .lsf and one .lsx path"
    )]
    UnsupportedPair { input: PathBuf, output: PathBuf },
    #[error("destination `{0}` already exists; pass --force to replace it")]
    DestinationExists(PathBuf),
    #[error("cannot {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Format(#[from] larian_formats::error::Error),
    #[error("cannot validate `{path}` before conversion: {source}")]
    XmlValidation {
        path: PathBuf,
        source: quick_xml::Error,
    },
}

#[derive(Clone, Copy, Debug)]
enum Direction {
    LsfToLsx,
    LsxToLsf,
}

/// Converts into a temporary sibling and publishes the result only after success.
pub fn convert(options: &Options) -> Result<(), Error> {
    let direction = Direction::from_paths(&options.source, &options.destination)?;
    if !options.force
        && options
            .destination
            .try_exists()
            .map_err(|source| Error::Io {
                operation: "inspect",
                path: options.destination.clone(),
                source,
            })?
    {
        return Err(Error::DestinationExists(options.destination.clone()));
    }

    if matches!(direction, Direction::LsxToLsf) {
        validate_lsx(&options.source)?;
    }
    let mut input = open(&options.source)?;
    let data = match direction {
        Direction::LsfToLsx => LsfData::read_lsf(&mut input)?,
        Direction::LsxToLsf => LsfData::read_lsx(&mut input)?,
    };

    let destination_parent = options
        .destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = Builder::new()
        .prefix(".bg3-ls-convert-")
        .tempfile_in(destination_parent)
        .map_err(|source| Error::Io {
            operation: "create a temporary output beside",
            path: options.destination.clone(),
            source,
        })?;

    match direction {
        Direction::LsfToLsx => data.write_lsx(temporary.as_file_mut())?,
        Direction::LsxToLsf => data.write_lsf(temporary.as_file_mut())?,
    }
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|source| Error::Io {
            operation: "sync a temporary output for",
            path: options.destination.clone(),
            source,
        })?;

    if options.force {
        match fs::metadata(&options.destination) {
            Ok(metadata) => {
                fs::set_permissions(temporary.path(), metadata.permissions()).map_err(
                    |source| Error::Io {
                        operation: "preserve permissions for",
                        path: options.destination.clone(),
                        source,
                    },
                )?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(Error::Io {
                    operation: "inspect permissions for",
                    path: options.destination.clone(),
                    source,
                });
            }
        }
        temporary
            .persist(&options.destination)
            .map_err(|error| Error::Io {
                operation: "replace",
                path: options.destination.clone(),
                source: error.error,
            })?;
    } else {
        temporary
            .persist_noclobber(&options.destination)
            .map_err(|error| {
                if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::DestinationExists(options.destination.clone())
                } else {
                    Error::Io {
                        operation: "create",
                        path: options.destination.clone(),
                        source: error.error,
                    }
                }
            })?;
    }

    Ok(())
}

// larian-formats 0.8.1 still uses quick-xml 0.39. A complete pass with the
// fixed parser prevents its quadratic attribute and namespace paths from
// receiving unvalidated LSX input.
fn validate_lsx(path: &Path) -> Result<(), Error> {
    let mut reader = NsReader::from_reader(std::io::BufReader::new(open(path)?));
    let mut buffer = Vec::new();
    loop {
        let (_, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|source| Error::XmlValidation {
                path: path.to_path_buf(),
                source,
            })?;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    attribute.map_err(|source| Error::XmlValidation {
                        path: path.to_path_buf(),
                        source: source.into(),
                    })?;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn open(path: &Path) -> Result<File, Error> {
    File::open(path).map_err(|source| Error::Io {
        operation: "open",
        path: path.to_path_buf(),
        source,
    })
}

impl Direction {
    fn from_paths(source: &Path, destination: &Path) -> Result<Self, Error> {
        match (extension(source), extension(destination)) {
            (Some("lsf"), Some("lsx")) => Ok(Self::LsfToLsx),
            (Some("lsx"), Some("lsf")) => Ok(Self::LsxToLsf),
            _ => Err(Error::UnsupportedPair {
                input: source.to_path_buf(),
                output: destination.to_path_buf(),
            }),
        }
    }
}

fn extension(path: &Path) -> Option<&str> {
    path.extension()?.to_str()
}
