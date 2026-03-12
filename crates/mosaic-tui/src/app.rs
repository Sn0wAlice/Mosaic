use mosaic_core::header::{VaultHeader, VaultPrelude};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// TUI application screens.
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Unlock,
    Init,
    Dashboard,
}

/// Field focus for the Unlock screen.
#[derive(Debug, Clone, PartialEq)]
pub enum UnlockField {
    VaultPath,
    Password,
    MountButton,
    InitButton,
}

impl UnlockField {
    pub fn next(&self) -> Self {
        match self {
            Self::VaultPath => Self::Password,
            Self::Password => Self::MountButton,
            Self::MountButton => Self::InitButton,
            Self::InitButton => Self::VaultPath,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::VaultPath => Self::InitButton,
            Self::Password => Self::VaultPath,
            Self::MountButton => Self::Password,
            Self::InitButton => Self::MountButton,
        }
    }
}

/// Field focus for the Init screen.
#[derive(Debug, Clone, PartialEq)]
pub enum InitField {
    HeaderPath,
    VaultName,
    Password,
    Confirm,
    TileSize,
    CreateButton,
    CancelButton,
}

impl InitField {
    pub fn next(&self) -> Self {
        match self {
            Self::HeaderPath => Self::VaultName,
            Self::VaultName => Self::Password,
            Self::Password => Self::Confirm,
            Self::Confirm => Self::TileSize,
            Self::TileSize => Self::CreateButton,
            Self::CreateButton => Self::CancelButton,
            Self::CancelButton => Self::HeaderPath,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::HeaderPath => Self::CancelButton,
            Self::VaultName => Self::HeaderPath,
            Self::Password => Self::VaultName,
            Self::Confirm => Self::Password,
            Self::TileSize => Self::Confirm,
            Self::CreateButton => Self::TileSize,
            Self::CancelButton => Self::CreateButton,
        }
    }
}

/// Available tile sizes.
pub const TILE_SIZES: &[(u64, &str)] = &[
    (128, "128 MB"),
    (256, "256 MB"),
    (512, "512 MB"),
    (1024, "1 GB"),
];

/// Dashboard field focus.
#[derive(Debug, Clone, PartialEq)]
pub enum DashboardField {
    FileList,
    UnmountButton,
    RefreshButton,
}

impl DashboardField {
    pub fn next(&self) -> Self {
        match self {
            Self::FileList => Self::UnmountButton,
            Self::UnmountButton => Self::RefreshButton,
            Self::RefreshButton => Self::FileList,
        }
    }
}

/// Main application state.
pub struct App {
    pub screen: Screen,
    pub running: bool,

    // Unlock screen state
    pub unlock_field: UnlockField,
    pub vault_path: String,
    pub unlock_password: PasswordBuffer,
    pub unlock_error: Option<String>,

    // Init screen state
    pub init_field: InitField,
    pub init_header_path: String,
    pub init_vault_name: String,
    pub init_password: PasswordBuffer,
    pub init_confirm: PasswordBuffer,
    pub init_tile_size_idx: usize,
    pub init_error: Option<String>,
    pub init_creating: bool,

    // Dashboard state
    pub dashboard_field: DashboardField,
    pub mount_point: Option<String>,
    pub header: Option<VaultHeader>,
    pub prelude: Option<VaultPrelude>,
    pub key: Option<[u8; 32]>,
    pub file_list_scroll: usize,
    pub dashboard_error: Option<String>,

    // FUSE mount handle (kept alive while mounted)
    pub mount_handle: Option<fuser::BackgroundSession>,
}

/// A password buffer that zeroizes on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PasswordBuffer {
    buf: String,
}

impl PasswordBuffer {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
        }
    }

    pub fn push(&mut self, c: char) {
        self.buf.push(c);
    }

    pub fn pop(&mut self) {
        self.buf.pop();
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.buf.as_bytes()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.buf
    }

    pub fn clear(&mut self) {
        self.buf.zeroize();
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            screen: Screen::Unlock,
            running: true,

            unlock_field: UnlockField::VaultPath,
            vault_path: String::new(),
            unlock_password: PasswordBuffer::new(),
            unlock_error: None,

            init_field: InitField::HeaderPath,
            init_header_path: String::new(),
            init_vault_name: String::new(),
            init_password: PasswordBuffer::new(),
            init_confirm: PasswordBuffer::new(),
            init_tile_size_idx: 1, // Default 256 MB
            init_error: None,
            init_creating: false,

            dashboard_field: DashboardField::FileList,
            mount_point: None,
            header: None,
            prelude: None,
            key: None,
            file_list_scroll: 0,
            dashboard_error: None,

            mount_handle: None,
        }
    }

    pub fn quit(&mut self) {
        self.running = false;
    }

    pub fn goto_init(&mut self) {
        self.screen = Screen::Init;
        self.init_field = InitField::HeaderPath;
        self.init_error = None;
    }

    pub fn goto_unlock(&mut self) {
        self.screen = Screen::Unlock;
        self.unlock_field = UnlockField::VaultPath;
        self.unlock_error = None;
        self.unlock_password.clear();
    }

    pub fn goto_dashboard(&mut self) {
        self.screen = Screen::Dashboard;
        self.dashboard_field = DashboardField::FileList;
        self.dashboard_error = None;
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(ref mut key) = self.key {
            key.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let mut app = App::new();
        assert_eq!(app.screen, Screen::Unlock);

        app.goto_init();
        assert_eq!(app.screen, Screen::Init);

        app.goto_unlock();
        assert_eq!(app.screen, Screen::Unlock);

        app.goto_dashboard();
        assert_eq!(app.screen, Screen::Dashboard);
    }

    #[test]
    fn test_unlock_field_navigation() {
        let field = UnlockField::VaultPath;
        assert_eq!(field.next(), UnlockField::Password);
        assert_eq!(field.next().next(), UnlockField::MountButton);
        assert_eq!(field.next().next().next(), UnlockField::InitButton);
        assert_eq!(field.next().next().next().next(), UnlockField::VaultPath);
    }

    #[test]
    fn test_init_field_navigation() {
        let field = InitField::HeaderPath;
        let mut current = field;
        for _ in 0..7 {
            current = current.next();
        }
        assert_eq!(current, InitField::HeaderPath); // Full cycle
    }

    #[test]
    fn test_password_buffer_zeroize() {
        let mut pw = PasswordBuffer::new();
        pw.push('s');
        pw.push('e');
        pw.push('c');
        assert_eq!(pw.len(), 3);
        pw.clear();
        assert!(pw.is_empty());
    }
}
