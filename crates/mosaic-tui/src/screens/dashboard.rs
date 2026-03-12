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

    let pool_count = header.pool_index.len().max(1) as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2),          // mount info
            Constraint::Length(1),          // separator
            Constraint::Length(1),          // TILES header
            Constraint::Length(pool_count), // tile bars (at least 1 line)
            Constraint::Length(1),          // separator
            Constraint::Length(1),          // FILES header
            Constraint::Min(3),            // file list
            Constraint::Length(1),          // separator
            Constraint::Length(1),          // buttons
            Constraint::Length(1),          // error
        ])
        .split(inner);

    // Mount info
    let mount_info = if let Some(ref mp) = app.mount_point {
        format!("  Mount point: {}", mp)
    } else {
        "  Mount point: (not mounted)".to_string()
    };
    let vault_info = format!("  Vault:       {}", app.vault_path);

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
    if header.pool_index.is_empty() {
        buf.set_string(
            chunks[3].x + 4,
            chunks[3].y,
            "(no tiles)",
            Style::default().fg(Color::DarkGray),
        );
    } else {
        for (i, pool) in header.pool_index.iter().enumerate() {
            let bar_area = Rect {
                x: chunks[3].x,
                y: chunks[3].y + i as u16,
                width: chunks[3].width,
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

    // FILES header — position SIZE and MODIFIED columns dynamically
    let w = chunks[5].width as usize;
    let size_col = w.saturating_sub(22);
    let date_col = w.saturating_sub(12);

    buf.set_string(
        chunks[5].x + 2,
        chunks[5].y,
        "FILES",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    if size_col > 10 {
        buf.set_string(
            chunks[5].x + size_col as u16,
            chunks[5].y,
            "SIZE",
            Style::default().fg(Color::DarkGray),
        );
    }
    if date_col > 10 {
        buf.set_string(
            chunks[5].x + date_col as u16,
            chunks[5].y,
            "MODIFIED",
            Style::default().fg(Color::DarkGray),
        );
    }

    // File list
    let file_area = chunks[6];
    let entries: Vec<_> = header.file_index.entries.iter().collect();
    let visible_count = file_area.height as usize;
    let scroll = app
        .file_list_scroll
        .min(entries.len().saturating_sub(visible_count));

    if entries.is_empty() {
        buf.set_string(
            file_area.x + 4,
            file_area.y,
            "(empty vault — copy files to the mount point)",
            Style::default().fg(Color::DarkGray),
        );
    } else {
        for (i, (path, entry)) in entries.iter().skip(scroll).take(visible_count).enumerate() {
            let y = file_area.y + i as u16;
            let size_str = format_size(entry.size);
            let date_str = format_date(entry.modified_at);

            let name_style = if app.dashboard_field == DashboardField::FileList {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            // Truncate long file names to fit before the size column
            let max_name_len = size_col.saturating_sub(4);
            let display_path = if path.len() > max_name_len {
                format!("  ...{}", &path[path.len() - max_name_len + 5..])
            } else {
                format!("  {}", path)
            };
            buf.set_string(file_area.x, y, &display_path, name_style);

            // Size column (right-aligned relative to terminal width)
            let size_x = file_area.x + size_col as u16;
            if size_x < file_area.x + file_area.width {
                buf.set_string(size_x, y, &size_str, Style::default().fg(Color::DarkGray));
            }

            // Date column
            let date_x = file_area.x + date_col as u16;
            if date_x < file_area.x + file_area.width {
                buf.set_string(date_x, y, &date_str, Style::default().fg(Color::DarkGray));
            }
        }
    }

    // Separator
    render_separator(chunks[7], buf);

    // Buttons + help
    let unmount_focused = app.dashboard_field == DashboardField::UnmountButton;
    let refresh_focused = app.dashboard_field == DashboardField::RefreshButton;

    render_buttons(
        &[
            ("  Unmount & seal  ", unmount_focused),
            (" Refresh ", refresh_focused),
        ],
        chunks[8],
        buf,
    );

    // Append quit hint after buttons
    let hint_x = chunks[8].x + 40;
    if hint_x + 6 < chunks[8].x + chunks[8].width {
        buf.set_string(
            hint_x,
            chunks[8].y,
            "q:quit",
            Style::default().fg(Color::DarkGray),
        );
    }

    // Error
    if let Some(ref err) = app.dashboard_error {
        buf.set_string(
            chunks[9].x + 2,
            chunks[9].y,
            &format!("✗ {}", err),
            Style::default().fg(Color::Red),
        );
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

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn format_date(timestamp: u64) -> String {
    // Simple date formatting without external dep
    let secs_per_day = 86400u64;
    let days_since_epoch = timestamp / secs_per_day;
    // Approximate: count years and remaining days
    let mut year = 1970i64;
    let mut remaining = days_since_epoch as i64;

    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let months_days: &[i64] = if is_leap(year) {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &d in months_days {
        if remaining < d {
            break;
        }
        remaining -= d;
        month += 1;
    }
    let day = remaining + 1;

    format!("{:04}-{:02}-{:02}", year, month, day)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}
