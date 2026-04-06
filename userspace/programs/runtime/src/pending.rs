#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingFlag(bool);

impl PendingFlag {
    pub const fn new() -> Self {
        Self(false)
    }

    pub const fn armed() -> Self {
        Self(true)
    }

    pub fn set(&mut self) {
        self.0 = true;
    }

    pub fn clear(&mut self) {
        self.0 = false;
    }

    pub fn is_set(self) -> bool {
        self.0
    }

    pub fn take(&mut self) -> bool {
        let value = self.0;
        self.0 = false;
        value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingValue<T> {
    value: Option<T>,
}

impl<T> PendingValue<T> {
    pub const fn new() -> Self {
        Self { value: None }
    }

    pub const fn with(value: T) -> Self {
        Self { value: Some(value) }
    }

    pub fn replace(&mut self, value: T) -> Option<T> {
        self.value.replace(value)
    }

    pub fn clear(&mut self) {
        self.value = None;
    }

    pub fn as_ref(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub fn take(&mut self) -> Option<T> {
        self.value.take()
    }
}
