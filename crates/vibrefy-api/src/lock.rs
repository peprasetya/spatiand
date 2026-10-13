//! The panic key, and the lock behind it.
//!
//! Escape pauses, blanks the picture and locks. On the Deck that key arrives from the
//! controller as well as the keyboard, because Spatiand maps the **B button** to
//! `KEY_ESC` - so the panic button is already under a thumb without either side arranging
//! it.
//!
//! What the lock is for shapes what it is. Someone walked into the room; the picture has
//! to be gone *now*, and getting back in has to be quick but not free. So:
//!
//!  * **The server session is the root of trust.** The PIN is a convenience lock on top of
//!    it, not an independent secret.
//!  * **A freshly paired session may set a PIN with no old one.** Signing in again proves
//!    as much as the PIN does and more, so it is the way out of a forgotten PIN. There is
//!    no reset code to invent and no dead end to get stuck in.
//!  * **The PIN outlives sign-outs.** It is set once and changed from Settings, rather
//!    than being something to re-enter per session.
//!  * **Locked survives a restart.** A lock that a kill and relaunch defeats is a
//!    curtain, not a lock.
//!
//! The hash is Argon2id. That is more than a four-to-eight digit PIN can really justify -
//! the whole space is searchable whatever the hash - but it costs nothing here and means
//! the stored file never contains anything worth taking.

use argon2::Argon2;
use password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

/// What the PIN screen will accept.
pub const MIN_LENGTH: usize = 4;
pub const MAX_LENGTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinError {
    TooShort,
    TooLong,
    NotDigits,
}

impl std::fmt::Display for PinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "A PIN is at least {MIN_LENGTH} digits."),
            Self::TooLong => write!(f, "A PIN is at most {MAX_LENGTH} digits."),
            Self::NotDigits => write!(f, "A PIN is digits only."),
        }
    }
}

pub fn check_shape(pin: &str) -> Result<(), PinError> {
    if pin.len() < MIN_LENGTH {
        return Err(PinError::TooShort);
    }
    if pin.len() > MAX_LENGTH {
        return Err(PinError::TooLong);
    }
    if !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(PinError::NotDigits);
    }
    Ok(())
}

/// Hashes a PIN for storage. The salt is fresh every time, so setting the same PIN twice
/// does not produce the same string on disk.
pub fn hash(pin: &str) -> Result<String, PinError> {
    check_shape(pin)?;
    let salt = SaltString::generate(&mut rand_core::OsRng);
    Ok(Argon2::default()
        .hash_password(pin.as_bytes(), &salt)
        .expect("Argon2 with a generated salt cannot fail")
        .to_string())
}

/// Whether `pin` matches `stored`. A stored value that cannot be parsed is a no, not a
/// panic - a corrupted config should send someone to the sign-in screen, not crash them
/// out of a locked application.
pub fn verify(stored: &str, pin: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(pin.as_bytes(), &parsed)
        .is_ok()
}

/// Where the application is, as far as the lock is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Open,
    /// Panic was pressed, or the application started while locked.
    Locked,
}

/// Everything the lock screen needs to know, and nothing it does not.
#[derive(Debug, Clone)]
pub struct Lock {
    pub state: LockState,
    /// `None` means no PIN has ever been set, and the lock can only be a curtain.
    pin: Option<String>,
}

impl Lock {
    pub fn new(pin: Option<String>, locked: bool) -> Self {
        Self {
            state: if locked {
                LockState::Locked
            } else {
                LockState::Open
            },
            pin,
        }
    }

    pub fn has_pin(&self) -> bool {
        self.pin.is_some()
    }

    pub fn stored(&self) -> Option<&str> {
        self.pin.as_deref()
    }

    pub fn is_locked(&self) -> bool {
        self.state == LockState::Locked
    }

    /// The panic key. Always succeeds - there is no state in which hiding the picture is
    /// the wrong thing to do.
    pub fn panic(&mut self) {
        self.state = LockState::Locked;
    }

    /// Tries a PIN. With no PIN set the lock is a curtain and anything opens it, which is
    /// honest: it was never keeping anyone out.
    pub fn unlock(&mut self, attempt: &str) -> bool {
        let opened = match &self.pin {
            None => true,
            Some(stored) => verify(stored, attempt),
        };
        if opened {
            self.state = LockState::Open;
        }
        opened
    }

    /// Sets or changes the PIN.
    ///
    /// No old PIN is asked for, deliberately. Reaching Settings means already being past
    /// the lock, and a freshly paired session has proved more than a PIN ever could - so
    /// signing in again is the way out of a forgotten one.
    pub fn set_pin(&mut self, pin: &str) -> Result<(), PinError> {
        self.pin = Some(hash(pin)?);
        Ok(())
    }

    pub fn clear_pin(&mut self) {
        self.pin = None;
    }

    /// Signing in again clears the lock, because it has just proved who this is.
    pub fn on_signed_in(&mut self) {
        self.state = LockState::Open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_is_four_to_eight_digits() {
        assert_eq!(Ok(()), check_shape("1234"));
        assert_eq!(Ok(()), check_shape("12345678"));
        assert_eq!(Err(PinError::TooShort), check_shape("123"));
        assert_eq!(Err(PinError::TooLong), check_shape("123456789"));
        assert_eq!(Err(PinError::NotDigits), check_shape("12a4"));
    }

    #[test]
    fn a_hashed_pin_verifies_and_a_wrong_one_does_not() {
        let stored = hash("4821").expect("a valid pin");
        assert!(verify(&stored, "4821"));
        assert!(!verify(&stored, "4822"));
        assert!(!verify(&stored, ""));
    }

    /// Nothing worth taking ends up in the file, and the same PIN twice does not look the
    /// same twice.
    #[test]
    fn the_stored_form_contains_neither_the_pin_nor_a_repeat() {
        let once = hash("4821").unwrap();
        let twice = hash("4821").unwrap();
        assert!(!once.contains("4821"));
        assert_ne!(once, twice, "a fresh salt every time");
        assert!(verify(&twice, "4821"), "and both still verify");
    }

    /// A corrupted file sends someone to sign in again; it does not take the process down.
    #[test]
    fn an_unreadable_stored_hash_is_a_refusal_rather_than_a_crash() {
        assert!(!verify("", "1234"));
        assert!(!verify("not a hash at all", "1234"));
    }

    #[test]
    fn panic_locks_and_the_right_pin_opens() {
        let mut lock = Lock::new(Some(hash("4821").unwrap()), false);
        assert!(!lock.is_locked());
        lock.panic();
        assert!(lock.is_locked());
        assert!(!lock.unlock("0000"));
        assert!(lock.is_locked(), "a wrong pin leaves it shut");
        assert!(lock.unlock("4821"));
        assert!(!lock.is_locked());
    }

    /// With no PIN the lock is a curtain, and pretending otherwise would be a lie about
    /// what it protects against.
    #[test]
    fn with_no_pin_set_anything_opens_it() {
        let mut lock = Lock::new(None, true);
        assert!(lock.is_locked());
        assert!(lock.unlock(""));
        assert!(!lock.is_locked());
    }

    /// The way out of a forgotten PIN: sign in again, then set a new one with no old one
    /// asked for.
    #[test]
    fn signing_in_again_opens_the_lock_and_lets_a_new_pin_be_set() {
        let mut lock = Lock::new(Some(hash("4821").unwrap()), true);
        assert!(lock.is_locked());

        lock.on_signed_in();
        assert!(!lock.is_locked());

        lock.set_pin("9999").expect("no old pin is asked for");
        assert!(verify(lock.stored().unwrap(), "9999"));
        assert!(!verify(lock.stored().unwrap(), "4821"), "the old one is gone");
    }

    /// A lock that a relaunch defeats is a curtain. The flag is stored, so the application
    /// comes back locked.
    #[test]
    fn locked_survives_being_restarted() {
        let stored = hash("4821").unwrap();
        let mut lock = Lock::new(Some(stored.clone()), false);
        lock.panic();
        assert!(lock.is_locked());

        // What the next run reads back out of the config file.
        let after_restart = Lock::new(Some(stored), lock.is_locked());
        assert!(after_restart.is_locked());
    }

    #[test]
    fn a_pin_that_is_not_a_pin_is_refused_before_it_is_stored() {
        let mut lock = Lock::new(None, false);
        assert_eq!(Err(PinError::TooShort), lock.set_pin("12"));
        assert!(!lock.has_pin(), "nothing was stored");
    }
}
