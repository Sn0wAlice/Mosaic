mod app;
mod screens;
mod widgets;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tracing::{error, info};

use app::{App, DashboardField, InitField, Screen, UnlockField, TILE_SIZES};
use mosaic_core::header::{self, VaultHeader};
use mosaic_core::lock::VaultLock;
use mosaic_core::pool::PoolManager;
use mosaic_core::security;

#[derive(Parser)]
#[command(name = "mosaic", version = "0.1.0", about = "Encrypted tile-based virtual partition manager")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new vault
    Init {
        /// Path for the vault header file
        header_path: PathBuf,
    },
    /// Mount an existing vault
    Mount {
        /// Path to the vault header file
        header_path: PathBuf,
        /// Mount point directory
        mountpoint: PathBuf,
    },
    /// Display vault status without mounting
    Status {
        /// Path to the vault header file
        header_path: PathBuf,
    },
    /// Unmount and lock a vault
    Seal {
        /// Path to the vault header file
        header_path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Apply OS-level security hardening BEFORE any key material is created:
    // - Disable core dumps (RLIMIT_CORE = 0)
    // - Disable ptrace attachment (Linux only)
    security::harden_process();

    let cli = Cli::parse();

    let is_tui = cli.command.is_none();

    if is_tui {
        // In TUI mode, suppress ALL logging (including fuser's `log` crate)
        // to avoid corrupting the terminal UI
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_writer(io::sink)
            .init();
    } else {
        // In CLI mode, log to stderr normally
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_writer(io::stderr)
            .init();
    }

    match cli.command {
        Some(Commands::Init { header_path }) => cmd_init(&header_path).await,
        Some(Commands::Mount {
            header_path,
            mountpoint,
        }) => cmd_mount(&header_path, &mountpoint).await,
        Some(Commands::Status { header_path }) => cmd_status(&header_path),
        Some(Commands::Seal { header_path }) => cmd_seal(&header_path),
        None => run_tui().await,
    }
}

async fn cmd_init(header_path: &Path) -> Result<()> {
    // Check FUSE availability
    mosaic_fuse::check_fuse().context("FUSE check failed")?;

    println!("Creating new Mosaic vault at: {}", header_path.display());

    let name = prompt_line("Vault name: ")?;
    let password = rpassword::prompt_password("Password: ")?;
    let confirm = rpassword::prompt_password("Confirm password: ")?;

    if password != confirm {
        anyhow::bail!("Passwords do not match");
    }

    VaultHeader::init(header_path, password.as_bytes(), &name, 256)
        .context("Failed to create vault")?;

    println!("Vault created successfully.");
    Ok(())
}

async fn cmd_mount(header_path: &Path, mountpoint: &Path) -> Result<()> {
    mosaic_fuse::check_fuse().context("FUSE check failed")?;

    // Acquire lock
    let _lock = VaultLock::acquire(header_path)
        .map_err(|e| anyhow::anyhow!("{}", e))
        .context("Failed to lock vault")?;

    let password = rpassword::prompt_password("Password: ")?;

    let (header, key) = VaultHeader::open(header_path, password.as_bytes())
        .context("Failed to open vault")?;

    // Lock the derived key in physical memory to prevent swap
    security::mlock_key(&key);

    let prelude = header::read_prelude(header_path)?;
    let header_dir = header_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let pools = PoolManager::new(
        header_dir,
        header.metadata.tile_size_bytes,
        key,
        header.pool_index.clone(),
    );

    // Verify pool integrity
    let issues = pools.verify_integrity();
    if !issues.is_empty() {
        for issue in &issues {
            eprintln!("Warning: {}", issue);
        }
        eprintln!("Pool integrity issues found. Proceeding anyway...");
    }

    // Create mountpoint if needed
    if !mountpoint.exists() {
        std::fs::create_dir_all(mountpoint)?;
    }

    println!("Mounting vault at: {}", mountpoint.display());
    println!("Press Ctrl+C to unmount and seal.");

    let session = mosaic_fuse::fs::mount(
        header,
        prelude,
        pools,
        header_path.to_path_buf(),
        key,
        mountpoint,
    )
    .context("Failed to mount FUSE filesystem")?;

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    println!("\nUnmounting...");
    drop(session);
    // _lock is dropped here, releasing the lock file
    println!("Vault sealed.");
    Ok(())
}

fn cmd_status(header_path: &Path) -> Result<()> {
    let prelude = header::read_prelude(header_path).context("Failed to read vault header")?;

    println!("Mosaic Vault Status");
    println!("  Path:    {}", header_path.display());
    println!("  Version: {}", prelude.version);
    println!("  Argon2 m_cost: {} KB", prelude.argon2_m_cost);
    println!("  Argon2 t_cost: {}", prelude.argon2_t_cost);
    println!("  Argon2 p_cost: {}", prelude.argon2_p_cost);

    // Try to open with password for full status
    let password = rpassword::prompt_password("Password (Enter to skip): ")?;
    if !password.is_empty() {
        match VaultHeader::open(header_path, password.as_bytes()) {
            Ok((header, _)) => {
                println!("  Name:       {}", header.metadata.name);
                println!("  Tile size:  {} MB", header.metadata.tile_size_bytes / (1024 * 1024));
                println!("  Pools:      {}", header.pool_index.len());
                println!("  Files:      {}", header.file_index.entries.len());
                for pool in &header.pool_index {
                    let pct = if header.metadata.tile_size_bytes > 0 {
                        (pool.size_bytes as f64 / header.metadata.tile_size_bytes as f64) * 100.0
                    } else {
                        0.0
                    };
                    println!(
                        "    {} — {:>6.1}% ({:?})",
                        pool.filename, pct, pool.status
                    );
                }
            }
            Err(e) => {
                println!("  Could not decrypt: {}", e);
            }
        }
    }

    Ok(())
}

fn cmd_seal(header_path: &Path) -> Result<()> {
    println!("Seal operation: vault at {} will be locked.", header_path.display());
    println!("If a FUSE mount is active, terminate the mount process to seal.");
    Ok(())
}

fn prompt_line(prompt: &str) -> Result<String> {
    use std::io::Write;
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

// ── TUI mode ──────────────────────────────────────────────────────────────

async fn run_tui() -> Result<()> {
    // Check FUSE availability
    if let Err(e) = mosaic_fuse::check_fuse() {
        eprintln!("Warning: {}", e);
        eprintln!("FUSE mounting will not be available.");
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    // Set up signal handler for clean shutdown
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    {
        let flag = shutdown_flag.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                flag.store(true, Ordering::SeqCst);
            }
        });
    }

    let result = run_tui_loop(&mut terminal, &mut app, &shutdown_flag).await;

    // Clean shutdown: unmount if still mounted
    if app.mount_handle.is_some() {
        do_unmount(&mut app).await;
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    shutdown_flag: &Arc<AtomicBool>,
) -> Result<()> {
    loop {
        // Check for signal-based shutdown
        if shutdown_flag.load(Ordering::SeqCst) {
            if app.mount_handle.is_some() {
                do_unmount(app).await;
            }
            app.quit();
        }

        terminal.draw(|frame| {
            let area = frame.size();
            match app.screen {
                Screen::Unlock => screens::unlock::render(app, area, frame.buffer_mut()),
                Screen::Init => screens::init::render(app, area, frame.buffer_mut()),
                Screen::Dashboard => screens::dashboard::render(app, area, frame.buffer_mut()),
            }
        })?;

        if !app.running {
            break;
        }

        // Poll for events with a timeout for dashboard refresh
        let timeout = if app.screen == Screen::Dashboard {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(100)
        };

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                handle_key(app, key.code, key.modifiers).await?;
            }
        } else if app.screen == Screen::Dashboard {
            // Auto-refresh dashboard
            refresh_dashboard(app);
        }
    }

    Ok(())
}

async fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // Global quit
    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        if app.mount_handle.is_some() {
            do_unmount(app).await;
        }
        app.quit();
        return Ok(());
    }

    match app.screen {
        Screen::Unlock => handle_unlock_key(app, code).await?,
        Screen::Init => handle_init_key(app, code).await?,
        Screen::Dashboard => handle_dashboard_key(app, code).await?,
    }

    Ok(())
}

async fn handle_unlock_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('q') if app.unlock_field != UnlockField::VaultPath && app.unlock_field != UnlockField::Password => {
            app.quit();
        }
        KeyCode::Tab => {
            app.unlock_field = app.unlock_field.next();
        }
        KeyCode::BackTab => {
            app.unlock_field = app.unlock_field.prev();
        }
        KeyCode::Enter => match app.unlock_field {
            UnlockField::MountButton => {
                do_mount(app).await;
            }
            UnlockField::InitButton => {
                app.goto_init();
            }
            UnlockField::VaultPath | UnlockField::Password => {
                app.unlock_field = app.unlock_field.next();
            }
        },
        KeyCode::Char(c) => match app.unlock_field {
            UnlockField::VaultPath => app.vault_path.push(c),
            UnlockField::Password => app.unlock_password.push(c),
            _ => {}
        },
        KeyCode::Backspace => match app.unlock_field {
            UnlockField::VaultPath => {
                app.vault_path.pop();
            }
            UnlockField::Password => {
                app.unlock_password.pop();
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

async fn handle_init_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Tab => {
            app.init_field = app.init_field.next();
        }
        KeyCode::BackTab => {
            app.init_field = app.init_field.prev();
        }
        KeyCode::Left if app.init_field == InitField::TileSize => {
            if app.init_tile_size_idx > 0 {
                app.init_tile_size_idx -= 1;
            }
        }
        KeyCode::Right if app.init_field == InitField::TileSize => {
            if app.init_tile_size_idx < TILE_SIZES.len() - 1 {
                app.init_tile_size_idx += 1;
            }
        }
        KeyCode::Enter => match app.init_field {
            InitField::CreateButton => {
                do_create_vault(app).await;
            }
            InitField::CancelButton => {
                app.goto_unlock();
            }
            _ => {
                app.init_field = app.init_field.next();
            }
        },
        KeyCode::Esc => {
            app.goto_unlock();
        }
        KeyCode::Char(c) => match app.init_field {
            InitField::HeaderPath => app.init_header_path.push(c),
            InitField::VaultName => app.init_vault_name.push(c),
            InitField::Password => app.init_password.push(c),
            InitField::Confirm => app.init_confirm.push(c),
            _ => {}
        },
        KeyCode::Backspace => match app.init_field {
            InitField::HeaderPath => {
                app.init_header_path.pop();
            }
            InitField::VaultName => {
                app.init_vault_name.pop();
            }
            InitField::Password => {
                app.init_password.pop();
            }
            InitField::Confirm => {
                app.init_confirm.pop();
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

async fn handle_dashboard_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('q') => {
            do_unmount(app).await;
            app.quit();
        }
        KeyCode::Tab => {
            app.dashboard_field = app.dashboard_field.next();
        }
        KeyCode::Enter => match app.dashboard_field {
            DashboardField::UnmountButton => {
                do_unmount(app).await;
                app.goto_unlock();
            }
            DashboardField::RefreshButton => {
                refresh_dashboard(app);
            }
        },
        _ => {}
    }
    Ok(())
}

async fn do_mount(app: &mut App) {
    app.unlock_error = None;

    let header_path = PathBuf::from(&app.vault_path);
    if !header_path.exists() {
        app.unlock_error = Some("File not found".to_string());
        return;
    }

    // Acquire lock before opening
    let lock = match VaultLock::acquire(&header_path) {
        Ok(l) => l,
        Err(e) => {
            app.unlock_error = Some(format!("{}", e));
            return;
        }
    };

    let password = app.unlock_password.as_bytes().to_vec();
    match VaultHeader::open(&header_path, &password) {
        Ok((header, key)) => {
            // Lock the derived key in physical memory to prevent swap
            security::mlock_key(&key);
            let prelude = match header::read_prelude(&header_path) {
                Ok(p) => p,
                Err(e) => {
                    app.unlock_error = Some(format!("{}", e));
                    return;
                }
            };

            let header_dir = header_path
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf();

            let pools = PoolManager::new(
                header_dir,
                header.metadata.tile_size_bytes,
                key,
                header.pool_index.clone(),
            );

            // Verify pool integrity
            let issues = pools.verify_integrity();
            for issue in &issues {
                app.push_error(format!("Pool: {}", issue));
            }

            // Compute space info
            let total = pools.total_capacity();
            let used = pools.total_used();
            app.total_space_bytes = total;
            app.used_space_bytes = used;
            app.free_space_bytes = total.saturating_sub(used);

            // Create a temporary mountpoint with random suffix
            let rand_suffix: String = (0..10)
                .map(|_| {
                    let idx = rand::random::<u8>() % 36;
                    if idx < 10 { (b'0' + idx) as char } else { (b'a' + idx - 10) as char }
                })
                .collect();
            let mount_point = std::env::temp_dir().join(format!("mosaic-{}", rand_suffix));
            if !mount_point.exists() {
                if let Err(e) = std::fs::create_dir_all(&mount_point) {
                    app.unlock_error = Some(format!("Failed to create mountpoint: {}", e));
                    return;
                }
            }

            // Mount FUSE
            match mosaic_fuse::fs::mount(
                header.clone(),
                prelude.clone(),
                pools,
                header_path.clone(),
                key,
                &mount_point,
            ) {
                Ok(handle) => {
                    app.mount_point = Some(mount_point.to_string_lossy().to_string());
                    app.header = Some(header);
                    app.prelude = Some(prelude);
                    app.key = Some(key);
                    app.mount_handle = Some(handle);
                    app.vault_lock = Some(lock);
                    app.unlock_password.clear();
                    app.goto_dashboard();
                    info!("Vault mounted at {}", mount_point.display());
                }
                Err(e) => {
                    app.unlock_error = Some(format!("Mount failed: {}", e));
                }
            }
        }
        Err(e) => {
            app.unlock_error = Some(format!("{}", e));
        }
    }
}

async fn do_create_vault(app: &mut App) {
    app.init_error = None;

    if app.init_header_path.is_empty() {
        app.init_error = Some("Header path is required".to_string());
        return;
    }
    if app.init_vault_name.is_empty() {
        app.init_error = Some("Vault name is required".to_string());
        return;
    }
    if app.init_password.is_empty() {
        app.init_error = Some("Password is required".to_string());
        return;
    }
    if app.init_password.as_str() != app.init_confirm.as_str() {
        app.init_error = Some("Passwords do not match".to_string());
        return;
    }

    let (tile_size_mb, _) = TILE_SIZES[app.init_tile_size_idx];
    let path = PathBuf::from(&app.init_header_path);

    app.init_creating = true;

    // Run init in a blocking task since Argon2 is CPU-intensive
    let password = app.init_password.as_bytes().to_vec();
    let name = app.init_vault_name.clone();

    let result = tokio::task::spawn_blocking(move || {
        VaultHeader::init(&path, &password, &name, tile_size_mb)
    })
    .await;

    app.init_creating = false;

    match result {
        Ok(Ok(())) => {
            app.vault_path = app.init_header_path.clone();
            app.init_password.clear();
            app.init_confirm.clear();
            app.goto_unlock();
        }
        Ok(Err(e)) => {
            app.init_error = Some(format!("{}", e));
        }
        Err(e) => {
            app.init_error = Some(format!("Internal error: {}", e));
        }
    }
}

async fn do_unmount(app: &mut App) {
    // Save header before unmounting
    if let (Some(ref header), Some(ref prelude), Some(ref key)) =
        (&app.header, &app.prelude, &app.key)
    {
        let header_path = PathBuf::from(&app.vault_path);
        if let Err(e) = header.save(&header_path, key, prelude) {
            error!("Failed to save header: {}", e);
            app.push_error(format!("Failed to save: {}", e));
        }
    }

    // Drop mount handle to unmount
    app.mount_handle.take();
    app.header = None;
    app.prelude = None;
    if let Some(ref mut key) = app.key {
        // Unlock from physical memory pinning, then zeroize
        security::munlock_key(key);
        zeroize::Zeroize::zeroize(key);
    }
    app.key = None;
    app.mount_point = None;

    // Release the vault lock
    app.vault_lock.take();

    info!("Vault unmounted and sealed");
}

fn refresh_dashboard(app: &mut App) {
    // Update space info from cached header
    if let Some(ref header) = app.header {
        let tile_size = header.metadata.tile_size_bytes;
        let pool_count = header.pool_index.len() as u64;
        let total = pool_count * tile_size;
        let used: u64 = header.pool_index.iter().map(|p| p.size_bytes).sum();
        app.total_space_bytes = total;
        app.used_space_bytes = used;
        app.free_space_bytes = total.saturating_sub(used);
    }
}
