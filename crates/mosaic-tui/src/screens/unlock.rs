use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::app::{App, UnlockField};

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    // Outer block
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![
            Span::styled("  ◈ MOSAIC", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]))
        .title_alignment(Alignment::Left)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    block.render(area, buf);

    // Version in top-right
    let version = "v0.1.0";
    if area.width > version.len() as u16 + 4 {
        buf.set_string(
            area.x + area.width - version.len() as u16 - 2,
            area.y,
            version,
            Style::default().fg(Color::DarkGray),
        );
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(1), // spacer
            Constraint::Length(1), // vault path label + input
            Constraint::Length(1), // spacer
            Constraint::Length(1), // password label + input
            Constraint::Length(1), // spacer
            Constraint::Length(1), // buttons
            Constraint::Length(1), // spacer
            Constraint::Length(1), // error message
            Constraint::Length(1), // spacer
            Constraint::Length(1), // help line
            Constraint::Min(0),   // remaining space
        ])
        .split(inner);

    // Vault path
    render_input_field(
        "Vault path: ",
        &app.vault_path,
        false,
        app.unlock_field == UnlockField::VaultPath,
        chunks[1],
        buf,
    );

    // Password
    render_input_field(
        "Password:   ",
        &"*".repeat(app.unlock_password.len()),
        true,
        app.unlock_field == UnlockField::Password,
        chunks[3],
        buf,
    );

    // Buttons
    render_buttons(
        &[
            ("  Mount  ", app.unlock_field == UnlockField::MountButton),
            (" Init new vault ", app.unlock_field == UnlockField::InitButton),
        ],
        chunks[5],
        buf,
    );

    // Error message
    if let Some(ref err) = app.unlock_error {
        let error = Paragraph::new(Line::from(Span::styled(
            format!("  ✗ {}", err),
            Style::default().fg(Color::Red),
        )));
        error.render(chunks[7], buf);
    }

    // Help line
    let help = Paragraph::new(Line::from(vec![
        Span::styled("  Tab", Style::default().fg(Color::Yellow)),
        Span::raw(": switch field  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(": confirm  "),
        Span::styled("q", Style::default().fg(Color::Yellow)),
        Span::raw(": quit"),
    ]))
    .style(Style::default().fg(Color::DarkGray));
    help.render(chunks[9], buf);
}

fn render_input_field(
    label: &str,
    value: &str,
    _is_password: bool,
    focused: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    let label_width = label.len() as u16;
    let label_style = Style::default().fg(Color::White);
    buf.set_string(area.x + 2, area.y, label, label_style);

    let input_start = area.x + 2 + label_width;
    let input_width = area.width.saturating_sub(label_width + 4) as usize;

    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    buf.set_string(input_start, area.y, "[", border_style);

    let display_value = if value.len() > input_width.saturating_sub(2) {
        &value[value.len() - (input_width.saturating_sub(2))..]
    } else {
        value
    };
    buf.set_string(
        input_start + 1,
        area.y,
        display_value,
        Style::default().fg(Color::White),
    );

    // Cursor position
    if focused {
        let cursor_x = input_start + 1 + display_value.len() as u16;
        if cursor_x < input_start + input_width as u16 {
            buf.set_string(cursor_x, area.y, "▎", Style::default().fg(Color::Cyan));
        }
    }

    // Padding and closing bracket
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
