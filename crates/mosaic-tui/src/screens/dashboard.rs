use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use crate::app::{App, DashboardField};
use crate::widgets::pool_bar::PoolBar;

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let header = match &app.header {
        Some(h) => h,
        None => return,
    };

    let vault_name = &header.metadata.name;
    let title = format!("  ◈ MOSAIC — {}  [MOUNTED]", vault_name);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![Span::styled(
            &title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]))
        .title_alignment(Alignment::Left)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    block.render(area, buf);

    // Determine how many error history lines to show (max 3)
    let error_lines = std::cmp::min(app.error_history.len(), 3) as u16;
    let error_section_height = if error_lines > 0 { error_lines } else { 1 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // mount info + space info
            Constraint::Length(1), // separator
            Constraint::Length(1), // TILES header
            Constraint::Min(3),   // tile bars (takes all remaining space)
            Constraint::Length(1), // separator
            Constraint::Length(1), // buttons
            Constraint::Length(error_section_height), // error history
        ])
        .split(inner);

    // Mount info + space info
    let mount_info = if let Some(ref mp) = app.mount_point {
        format!("  Mount point: {}", mp)
    } else {
        "  Mount point: (not mounted)".to_string()
    };
    let vault_info = format!("  Vault:       {}", app.vault_path);
    let space_info = format!(
        "  Space:       {} used / {} total ({} free)",
        format_size(app.used_space_bytes),
        format_size(app.total_space_bytes),
        format_size(app.free_space_bytes),
    );

    buf.set_string(
        chunks[0].x,
        chunks[0].y,
        &mount_info,
        Style::default().fg(Color::White),
    );
    buf.set_string(
        chunks[0].x,
        chunks[0].y + 1,
        &vault_info,
        Style::default().fg(Color::DarkGray),
    );
    buf.set_string(
        chunks[0].x,
        chunks[0].y + 2,
        &space_info,
        Style::default().fg(Color::Green),
    );

    // Separator
    render_separator(chunks[1], buf);

    // TILES header
    buf.set_string(
        chunks[2].x + 2,
        chunks[2].y,
        "TILES",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    // Pool bars
    let tile_area = chunks[3];
    if header.pool_index.is_empty() {
        buf.set_string(
            tile_area.x + 4,
            tile_area.y,
            "(no tiles)",
            Style::default().fg(Color::DarkGray),
        );
    } else {
        for (i, pool) in header.pool_index.iter().enumerate() {
            if i as u16 >= tile_area.height {
                break;
            }
            let bar_area = Rect {
                x: tile_area.x,
                y: tile_area.y + i as u16,
                width: tile_area.width,
                height: 1,
            };
            let pool_bar = PoolBar::new(
                pool.id,
                pool.filename.clone(),
                pool.size_bytes,
                header.metadata.tile_size_bytes,
                pool.status.clone(),
            );
            pool_bar.render(bar_area, buf);
        }
    }

    // Separator
    render_separator(chunks[4], buf);

    // Buttons + help
    let unmount_focused = app.dashboard_field == DashboardField::UnmountButton;
    let refresh_focused = app.dashboard_field == DashboardField::RefreshButton;

    render_buttons(
        &[
            ("  Unmount & seal  ", unmount_focused),
            (" Refresh ", refresh_focused),
        ],
        chunks[5],
        buf,
    );

    let hint_x = chunks[5].x + 40;
    if hint_x + 6 < chunks[5].x + chunks[5].width {
        buf.set_string(
            hint_x,
            chunks[5].y,
            "q:quit",
            Style::default().fg(Color::DarkGray),
        );
    }

    // Error history (show latest errors, most recent at bottom)
    let err_area = chunks[6];
    if !app.error_history.is_empty() {
        let start = if app.error_history.len() > 3 {
            app.error_history.len() - 3
        } else {
            0
        };
        for (i, err) in app.error_history.iter().skip(start).enumerate() {
            if i as u16 >= err_area.height {
                break;
            }
            let max_len = (err_area.width as usize).saturating_sub(4);
            let truncated = if err.len() > max_len {
                format!("✗ {}…", &err[..max_len.saturating_sub(1)])
            } else {
                format!("✗ {}", err)
            };
            buf.set_string(
                err_area.x + 2,
                err_area.y + i as u16,
                &truncated,
                Style::default().fg(Color::Red),
            );
        }
    } else if let Some(ref err) = app.dashboard_error {
        buf.set_string(
            err_area.x + 2,
            err_area.y,
            &format!("✗ {}", err),
            Style::default().fg(Color::Red),
        );
    }
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn render_separator(area: Rect, buf: &mut Buffer) {
    buf.set_string(
        area.x,
        area.y,
        &"─".repeat(area.width as usize),
        Style::default().fg(Color::DarkGray),
    );
}

fn render_buttons(buttons: &[(&str, bool)], area: Rect, buf: &mut Buffer) {
    let mut x = area.x + 2;
    for (label, focused) in buttons {
        let style = if *focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let bracket_style = if *focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        buf.set_string(x, area.y, "[", bracket_style);
        buf.set_string(x + 1, area.y, label, style);
        buf.set_string(x + 1 + label.len() as u16, area.y, "]", bracket_style);
        x += label.len() as u16 + 3;
    }
}
