use std::{env, error::Error, fmt};

pub const BOOT_MODE_ENV: &str = "SERVICEOS_BOOT_MODE";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootMode {
    Full,
    Reduced,
    Safe,
    Recovery,
}

impl BootMode {
    pub fn from_env_value(value: &str) -> Result<Self, InvalidBootMode> {
        match value {
            "full" => Ok(Self::Full),
            "reduced" => Ok(Self::Reduced),
            "safe" => Ok(Self::Safe),
            "recovery" => Ok(Self::Recovery),
            _ => Err(InvalidBootMode {
                value: value.to_owned(),
            }),
        }
    }

    pub const fn env_value(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Reduced => "reduced",
            Self::Safe => "safe",
            Self::Recovery => "recovery",
        }
    }

    pub const fn writes_bundle_note(self) -> bool {
        !matches!(self, Self::Full)
    }
}

pub fn selected_boot_mode() -> Result<Option<BootMode>, Box<dyn Error>> {
    let Some(value) = env::var_os(BOOT_MODE_ENV) else {
        return Ok(None);
    };
    let value = value.to_string_lossy();
    Ok(Some(BootMode::from_env_value(value.as_ref())?))
}

#[derive(Debug, Eq, PartialEq)]
pub struct InvalidBootMode {
    value: String,
}

impl fmt::Display for InvalidBootMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid {BOOT_MODE_ENV}={:?}; expected one of: full, reduced, safe, recovery",
            self.value
        )
    }
}

impl Error for InvalidBootMode {}

#[cfg(test)]
mod tests {
    use super::{selected_boot_mode, BootMode, BOOT_MODE_ENV};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parses_all_supported_modes() {
        assert_eq!(BootMode::from_env_value("full"), Ok(BootMode::Full));
        assert_eq!(BootMode::from_env_value("reduced"), Ok(BootMode::Reduced));
        assert_eq!(BootMode::from_env_value("safe"), Ok(BootMode::Safe));
        assert_eq!(BootMode::from_env_value("recovery"), Ok(BootMode::Recovery));
    }

    #[test]
    fn maps_all_modes_to_canonical_env_values() {
        assert_eq!(BootMode::Full.env_value(), "full");
        assert_eq!(BootMode::Reduced.env_value(), "reduced");
        assert_eq!(BootMode::Safe.env_value(), "safe");
        assert_eq!(BootMode::Recovery.env_value(), "recovery");
    }

    #[test]
    fn rejects_unknown_mode_with_loud_error() {
        let error = BootMode::from_env_value("mystery")
            .expect_err("unknown values should fail")
            .to_string();
        assert!(error.contains(BOOT_MODE_ENV));
        assert!(error.contains("full, reduced, safe, recovery"));
    }

    #[test]
    fn unset_env_selects_default_full_implicitly() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var(BOOT_MODE_ENV).ok();
        unsafe {
            std::env::remove_var(BOOT_MODE_ENV);
        }
        let selected = selected_boot_mode().expect("unset env should be accepted");
        unsafe {
            match previous {
                Some(value) => std::env::set_var(BOOT_MODE_ENV, value),
                None => std::env::remove_var(BOOT_MODE_ENV),
            }
        }
        assert_eq!(selected, None);
    }
}
