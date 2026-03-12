use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Widget},
};

use crate::app::{App, InitField, TILE_SIZES};

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![
            Span::styled(
                "  ◈ MOSAIC — New vault",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]))
        .title_alignment(Alignment::Left)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    block.render(area, buf);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(1), // spacer
            Constraint::Length(1), // header path
            Constraint::Length(1), // spacer
            Constraint::Length(1), // vault name
            Constraint::Length(1), // spacer
            Constraint::Length(1), // password
            Constraint::Length(1), // spacer
            Constraint::Length(1), // confirm
            Constraint::Length(1), // spacer
            Constraint::Length(1), // tile size
            Constraint::Length(1), // spacer
            Constraint::Length(1), // buttons
            Constraint::Length(1), // spacer
            Constraint::Length(1), // error / progress
            Constraint::Min(0),
        ])
        .split(inner);

    render_input(
        "Header path: ",
        &app.init_header_path,
        false,
        app.init_field == InitField::HeaderPath,
        chunks[1],
        buf,
    );

    render_input(
        "Vault name:  ",
        &app.init_vault_name,
        false,
        app.init_field == InitField::VaultName,
        chunks[3],
        buf,
    );

    render_input(
        "Password:    ",
        &"*".repeat(app.init_password.len()),
        true,
        app.init_field == InitField::Password,
        chunks[5],
        buf,
    );

    render_input(
        "Confirm:     ",
        &"*".repeat(app.init_confirm.len()),
        true,
        app.init_field == InitField::Confirm,
        chunks[7],
        buf,
    );

    // Tile size selector
    render_tile_selector(app, chunks[9], buf);

    // Buttons
    render_buttons(
        &[
            ("  Create  ", app.init_field == InitField::CreateButton),
            (" Cancel ", app.init_field == InitField::CancelButton),
        ],
        chunks[11],
        buf,
    );

    // Error or progress
    if app.init_creating {
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(Color::Cyan))
            .label("Deriving key (Argon2id)...")
            .ratio(0.5);
        gauge.render(chunks[13], buf);
    } else if let Some(ref err) = app.init_error {
        let error = Paragraph::new(Line::from(Span::styled(
            format!("  ✗ {}", err),
            Style::default().fg(Color::Red),
        )));
        error.render(chunks[13], buf);
    }
}

fn render_tile_selector(app: &App, area: Rect, buf: &mut Buffer) {
    let focused = app.init_field == InitField::TileSize;
    let label = "Tile size:   ";
    let label_width = label.len() as u16;

    buf.set_string(
        area.x + 2,
        area.y,
        label,
        Style::default().fg(Color::White),
    );

    let (_, size_label) = TILE_SIZES[app.init_tile_size_idx];
    let selector = format!("[ {} ▼ ]", size_label);

    let style = if focused {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    buf.set_string(area.x + 2 + label_width, area.y, &selector, style);

    if focused {
        let hint = " ←→ to change";
        buf.set_string(
            area.x + 2 + label_width + selector.len() as u16 + 1,
            area.y,
            hint,
            Style::default().fg(Color::DarkGray),
        );
    }
}

fn render_input(
    label: &str,
    value: &str,
    _is_password: bool,
    focused: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    let label_width = label.len() as u16;
    buf.set_string(
        area.x + 2,
        area.y,
        label,
        Style::default().fg(Color::White),
    );

    let input_start = area.x + 2 + label_width;
    let input_width = area.width.saturating_sub(label_width + 4) as usize;

    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    buf.set_string(input_start, area.y, "[", border_style);

    let display = if value.len() > input_width.saturating_sub(2) {
        &value[value.len() - (input_width.saturating_sub(2))..]
    } else {
        value
    };
    buf.set_string(
        input_start + 1,
        area.y,
        display,
        Style::default().fg(Color::White),
    );

    if focused {
        let cursor_x = input_start + 1 + display.len() as u16;
        if cursor_x < input_start + input_width as u16 {
            buf.set_string(cursor_x, area.y, "▎", Style::default().fg(Color::Cyan));
        }
    }

    let close_x = input_start + input_width as u16;
    if close_x < area.x + area.width {
        buf.set_string(close_x, area.y, "]", border_style);
    }
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
        x += label.len() as u16 + 4;
    }
}
