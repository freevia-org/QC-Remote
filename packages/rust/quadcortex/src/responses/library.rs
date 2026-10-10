use super::*;

/// One item in a preset, impulse-response, capture or recents/favorites listing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntry {
    /// Display name.
    pub name: String,
    /// Key that identifies the item on the device. For recents and favorites entries this is the item's folder key.
    pub key: String,
    /// Author recorded in the item's metadata; omitted when absent or empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Description recorded in the item's metadata; omitted when absent or empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Date string recorded in the item's metadata; omitted when absent or empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Product key of the item, when the device reports one.
    pub product_key: Option<String>,
    /// Product version metadata, a decimal string such as `"1"` or `"2"`.
    ///
    /// QC Remote loads a Neural Capture as model `14001` when this is `"2"`
    /// and as model `14000` when it is `"1"`, and does not load one that
    /// carries any other value. That is this project's own behaviour; what the
    /// versions mean on the device is not documented here. Across full library
    /// listings from CorOS 4.1.0 (about fourteen thousand entries) only `"1"`
    /// and `"2"` appeared, and only on user and cloud capture entries; presets,
    /// impulse responses and the factory capture folders carried none.
    pub version: Option<String>,
    /// Key of the folder that holds the item.
    pub folder_key: Option<String>,
    /// Display name of the folder that holds the item.
    pub folder_name: Option<String>,
    /// Index of the item within its folder as reported by the device. `None` when the reply gives no index.
    pub position: Option<u32>,
    /// Raw instrument enumeration value from the item's metadata, if present.
    ///
    /// Hardware listings have been observed to report `1` for items tagged as
    /// guitar and `2` for items tagged as bass; `0` or an absent field means
    /// the item is untagged. Values `3`, `4` and `5` are believed to be Synth,
    /// Vocal and Other, the order in which the unit's capture-save screen offers
    /// Guitar, Bass, Synth, Vocal and Other as the preferred instrument (CorOS
    /// 4.1.0). Values `4` and `5` have been seen rarely in full library listings
    /// (`4` on four factory presets, three of them named for vocal use, and one
    /// capture; `5` on one capture), which fits that reading; `3` has not been
    /// seen. The names for `3` to `5` have not been verified against the unit
    /// used for this project's tests.
    pub instrument: Option<i32>,
    /// Whether the item belongs to the factory library.
    pub is_factory: bool,
    /// Whether the item is flagged as a plugin item. Only recents and favorites replies set this; file listings always report `false`.
    pub is_plugin: bool,
}

/// A folder in a library listing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolder {
    /// Key that identifies the folder on the device.
    pub key: String,
    /// Display name of the folder.
    pub name: String,
    /// Key of the parent folder; omitted for a top-level folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    /// Whether this is a factory folder.
    pub is_factory: bool,
    /// Whether this is the downloads folder.
    pub is_downloads: bool,
    /// Whether this is a library folder.
    pub is_library: bool,
}

/// A decoded library listing: items and, when the listing includes them, their folders.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntries {
    /// The listed items.
    pub entries: Vec<LibraryEntry>,
    /// Folders the listing covers. The decoders in this module leave it empty; it is omitted from serialized output when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<LibraryFolder>,
}

/// Decodes a recents or favorites reply for presets.
///
/// The reply must echo `request_id` and be a preset list. Every item needs a name and a folder key. The returned
/// entries use the folder key as their `key` and carry no author, description, date, version, position or
/// instrument. Fails if the payload cannot be decoded, the message action is not UPDATE, the request id is
/// missing or different, the list is not of presets, or an item lacks a name or folder key.
pub fn decode_recents_favorites(
    payload: &[u8],
    request_id: u64,
) -> Result<LibraryEntries, ResponseDecodeError> {
    let message: pa::RecentsFavoritesMessage = decode_reply(payload)?;
    require_update_action(message.action, "recents/favorites action is not UPDATE")?;
    let actual_id = message
        .request_id
        .map(|pa::recents_favorites_message::RequestId::RequestId(value)| value)
        .ok_or(ResponseDecodeError::Incomplete(
            "recents/favorites request_id",
        ))?;
    if actual_id != request_id {
        return Err(ResponseDecodeError::Mismatch(
            "recents/favorites request_id does not match",
        ));
    }
    if message.r#type != pa::recents_favorites_message::RecentFavoriteType::Preset as i32 {
        return Err(ResponseDecodeError::Mismatch(
            "recents/favorites reply is not the requested PRESET list",
        ));
    }
    Ok(LibraryEntries {
        entries: message
            .items
            .into_iter()
            .map(|item| {
                if item.name.is_empty() || item.folder_key.is_empty() {
                    return Err(ResponseDecodeError::Incomplete(
                        "recents/favorites item identity",
                    ));
                }
                Ok(LibraryEntry {
                    key: item.folder_key.clone(),
                    name: item.name,
                    author: None,
                    description: None,
                    date: None,
                    product_key: (!item.product_key.is_empty()).then_some(item.product_key),
                    version: None,
                    folder_key: Some(item.folder_key),
                    folder_name: (!item.folder_name.is_empty()).then_some(item.folder_name),
                    position: None,
                    instrument: None,
                    is_factory: item.is_factory,
                    is_plugin: item.is_plugin,
                })
            })
            .collect::<Result<_, ResponseDecodeError>>()?,
        folders: Vec::new(),
    })
}

/// Favorites uses the shared preset-library envelope while retaining a
/// command-specific projection in the native runtime.
pub fn decode_favorites(
    payload: &[u8],
    request_id: u64,
) -> Result<LibraryEntries, ResponseDecodeError> {
    decode_recents_favorites(payload, request_id)
}

/// Decodes the pinned models and captures. Fails only if the payload cannot be decoded.
pub fn decode_pinned_models(payload: &[u8]) -> Result<PinnedModels, ResponseDecodeError> {
    let message: pa::PinnedModelsMessage = decode_reply(payload)?;
    Ok(PinnedModels {
        models: message.models,
        captures: message.captures,
    })
}

/// Decodes a file-listing reply for the folder `folder_key`.
///
/// When `request_id` is `Some`, the reply must echo it. The reply's folder key must equal `folder_key`, ignoring
/// trailing slashes. Every file needs a key and a name, and a file position must not be negative. Fails with
/// [`ResponseDecodeError::Incomplete`] or [`ResponseDecodeError::Mismatch`] otherwise, or if the payload cannot
/// be decoded.
pub fn decode_library_files(
    payload: &[u8],
    request_id: Option<u64>,
    folder_key: &str,
) -> Result<LibraryEntries, ResponseDecodeError> {
    decode_library_files_message(decode_reply(payload)?, request_id, folder_key)
}

/// Decode a typed Impulse Response listing while retaining FileMessage's
/// action flexibility. The schema makes type optional, but an explicitly
/// different type cannot satisfy an IR request.
pub fn decode_ir_files(
    payload: &[u8],
    request_id: u64,
    folder_key: &str,
) -> Result<LibraryEntries, ResponseDecodeError> {
    let message: pa::FileMessage = decode_reply(payload)?;
    if message.r#type.is_some_and(|value| match value {
        pa::file_message::Type::Type(value) => value != 1,
    }) {
        return Err(ResponseDecodeError::Mismatch(
            "file listing is not an Impulse Response list",
        ));
    }
    decode_library_files_message(message, Some(request_id), folder_key)
}

fn decode_library_files_message(
    message: pa::FileMessage,
    request_id: Option<u64>,
    folder_key: &str,
) -> Result<LibraryEntries, ResponseDecodeError> {
    // File replies are broadcasts: CREATE, UPDATE, DELETE, and READ actions
    // all arrive on the same channel without a blanket gate, so folder
    // identity (plus request_id for typed reads) is authoritative here.
    let actual_id = message
        .request_id
        .map(|pa::file_message::RequestId::RequestId(value)| value);
    if let Some(request_id) = request_id {
        if actual_id != Some(request_id) {
            return Err(ResponseDecodeError::Mismatch(
                "file listing request_id does not match",
            ));
        }
    }
    let folder = message
        .folder
        .map(|pa::file_message::Folder::Folder(value)| value)
        .ok_or(ResponseDecodeError::Incomplete("file listing folder"))?;
    let actual_key = folder
        .key
        .as_ref()
        .map(|pa::folder_info::Key::Key(value)| value.clone())
        .ok_or(ResponseDecodeError::Incomplete("file listing folder key"))?;
    if actual_key.trim_end_matches('/') != folder_key.trim_end_matches('/') {
        return Err(ResponseDecodeError::Mismatch(
            "file listing folder does not match",
        ));
    }
    Ok(library_entries_from_folder(folder)?.1)
}

/// Decode one broadcast File folder push into its canonical key and its full
/// library entries, without the caller having to already know which folder the
/// frame carries.
///
/// The QC publishes its whole directory as a burst of independent folder
/// frames, so a host caching that publication learns each key from the frame
/// itself. Returns `Ok(None)` for a File push that carries no folder or no
/// folder key, which is not an error: CREATE/UPDATE/DELETE pushes share this
/// message type.
pub fn decode_library_folder(
    payload: &[u8],
) -> Result<Option<(String, LibraryEntries)>, ResponseDecodeError> {
    let message: pa::FileMessage = decode_reply(payload)?;
    let Some(folder) = message
        .folder
        .map(|pa::file_message::Folder::Folder(value)| value)
    else {
        return Ok(None);
    };
    if folder
        .key
        .as_ref()
        .map(|pa::folder_info::Key::Key(value)| value.is_empty())
        .unwrap_or(true)
    {
        return Ok(None);
    }
    let (key, entries) = library_entries_from_folder(folder)?;
    Ok(Some((key.trim_end_matches('/').to_string(), entries)))
}

/// Project one decoded folder into library entries, returning the folder's own
/// key alongside them. Shared by the correlated read path and the cached
/// directory publication so both produce byte-identical entries.
fn library_entries_from_folder(
    folder: pa::FolderInfo,
) -> Result<(String, LibraryEntries), ResponseDecodeError> {
    let actual_key = folder
        .key
        .map(|pa::folder_info::Key::Key(value)| value)
        .ok_or(ResponseDecodeError::Incomplete("file listing folder key"))?;
    let is_factory = folder
        .is_factory
        .map(|pa::folder_info::IsFactory::IsFactory(value)| value)
        .unwrap_or(false);
    let folder_name = folder
        .name
        .map(|pa::folder_info::Name::Name(value)| value)
        .unwrap_or_else(|| {
            actual_key
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string()
        });
    let entries = LibraryEntries {
        entries: folder
            .files
            .into_iter()
            .map(|file| {
                let key = file
                    .key
                    .map(|pa::product_data::Key::Key(value)| value)
                    .filter(|value| !value.is_empty())
                    .ok_or(ResponseDecodeError::Incomplete("file key"))?;
                let name = file
                    .name
                    .map(|pa::product_data::Name::Name(value)| value)
                    .filter(|value| !value.is_empty())
                    .ok_or(ResponseDecodeError::Incomplete("file name"))?;
                let position = file
                    .index
                    .map(|pa::product_data::Index::Index(value)| value)
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| ResponseDecodeError::Mismatch("negative file position"))?;
                let instrument = file
                    .instrument
                    .map(|pa::product_data::Instrument::Instrument(value)| value);
                let version = file
                    .version
                    .map(|pa::product_data::Version::Version(value)| value)
                    .filter(|value| !value.is_empty());
                Ok(LibraryEntry {
                    name,
                    product_key: Some(key.clone()),
                    key,
                    author: file
                        .author
                        .map(|pa::product_data::Author::Author(value)| value)
                        .filter(|value| !value.is_empty()),
                    description: file
                        .description
                        .map(|pa::product_data::Description::Description(value)| value)
                        .filter(|value| !value.is_empty()),
                    date: file
                        .date
                        .map(|pa::product_data::Date::Date(value)| value)
                        .filter(|value| !value.is_empty()),
                    version,
                    folder_key: Some(actual_key.clone()),
                    folder_name: Some(folder_name.clone()),
                    position,
                    instrument,
                    is_factory,
                    is_plugin: false,
                })
            })
            .collect::<Result<_, ResponseDecodeError>>()?,
        folders: Vec::new(),
    };
    Ok((actual_key, entries))
}
