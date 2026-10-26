//! The Mac keychain for the AI tab (`poni_core::usage::Want::Keychain`).
//! A placeholder so the worker builds until the mac lane's reader lands
//! (its file replaces this one): it finds nothing.

use poni_core::usage::Got;

/// A generic password by service (and account): `Got::Text` when found,
/// `Got::Missing` when not there, `Got::Denied` when the person said no.
pub fn read(_service: &str, _account: Option<&str>) -> Got {
    Got::Missing
}
