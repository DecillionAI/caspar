//! Translation of `chain/common/trilean.go`.

/// A boolean that can also be undefined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum Trilean {
    /// The value has not been defined yet.
    Undefined = 0,
    /// The value is defined and true.
    True = 1,
    /// The value is defined and false.
    False = 2,
}

impl Default for Trilean {
    fn default() -> Self {
        Trilean::Undefined
    }
}

impl std::fmt::Display for Trilean {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Trilean::Undefined => "Undefined",
            Trilean::True => "True",
            Trilean::False => "False",
        };
        write!(f, "{}", s)
    }
}
