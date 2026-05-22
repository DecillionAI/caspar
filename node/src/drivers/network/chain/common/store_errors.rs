//! Translation of `chain/common/store_errors.go`.

/// Encodes the nature of a [`StoreErr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum StoreErrType {
    /// An item is not found.
    KeyNotFound = 0,
    /// An item is no longer in the store because it was evicted from the cache.
    TooLate = 1,
    /// An attempt was made to insert a non-sequential item in a cache.
    SkippedIndex = 2,
    /// An attempt was made to retrieve objects associated to a non-existent
    /// participant.
    UnknownParticipant = 3,
    /// A cache is empty.
    Empty = 4,
    /// An attempt was made to insert an item already present in a cache.
    KeyAlreadyExists = 5,
}

/// A generic error type that encodes errors when accessing objects in the
/// hashgraph store.
#[derive(Debug, Clone)]
pub struct StoreErr {
    pub data_type: String,
    pub err_type: StoreErrType,
    pub key: String,
}

/// Creates a [`StoreErr`] pertaining to an object identified by its `data_type`
/// and `key`. The `err_type` determines the nature of the error.
pub fn new_store_err(data_type: &str, err_type: StoreErrType, key: &str) -> StoreErr {
    StoreErr {
        data_type: data_type.to_string(),
        err_type,
        key: key.to_string(),
    }
}

impl StoreErr {
    pub fn new(data_type: &str, err_type: StoreErrType, key: &str) -> StoreErr {
        new_store_err(data_type, err_type, key)
    }
}

impl std::fmt::Display for StoreErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = match self.err_type {
            StoreErrType::KeyNotFound => "Not Found",
            StoreErrType::TooLate => "Too Late",
            StoreErrType::SkippedIndex => "Skipped Index",
            StoreErrType::UnknownParticipant => "Unknown Participant",
            StoreErrType::Empty => "Empty",
            StoreErrType::KeyAlreadyExists => "Key Already Exists",
        };
        write!(f, "{}, {}, {}", self.data_type, self.key, m)
    }
}

impl std::error::Error for StoreErr {}

/// Checks that an error is a [`StoreErr`] and that its code matches the
/// provided [`StoreErrType`].
pub fn is_store(err: &anyhow::Error, t: StoreErrType) -> bool {
    err.downcast_ref::<StoreErr>()
        .map(|store_err| store_err.err_type == t)
        .unwrap_or(false)
}
