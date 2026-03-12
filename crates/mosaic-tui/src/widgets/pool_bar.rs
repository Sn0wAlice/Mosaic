use mosaic_core::header::PoolStatus;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};

/// Widget for displaying the status of a tile pool.
pub struct PoolBar {
    #[allow(dead_code)]
    pub pool_id: u32,
    pub filename: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub status: PoolStatus,
}

impl PoolBar {
    pub fn new(
        pool_id: u32,
        filename: String,
        used_bytes: u64,
        total_bytes: u64,
        status: PoolStatus,
    ) -> Self {
        Self {
            pool_id,
            filename,
            used_bytes,
            total_bytes,
            status,
        }
    }

    fn percentage(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.used_bytes as f64 / self.total_bytes as f64) * 100.0
    }
}

impl Widget for PoolBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 20 {
            return;
        }

        let pct = self.percentage();

        match self.status {
            PoolStatus::Pending => {
                let text = format!("  {}  (pending)", self.filename);
                let style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
                buf.set_string(area.x, area.y, &text, style);
                return;
            }
            _ => {}
        }

        // Label: "pool_000  "
        let label = format!("  {}  ", self.filename);
        let label_width = label.len() as u16;
        buf.set_string(area.x, area.y, &label, Style::default().fg(Color::Cyan));

        // Progress bar
        let pct_label = format!(" {:>3.0}%", pct);
        let pct_width = pct_label.len() as u16;

        let bar_start = area.x + label_width;
        let bar_end = area.x + area.width - pct_width - 1;
        if bar_end <= bar_start {
            return;
        }
        let bar_width = (bar_end - bar_start) as usize;

        let filled = ((pct / 100.0) * bar_width as f64) as usize;
        let empty = bar_width.saturating_sub(filled);

        let bar_color = if pct > 90.0 {
            Color::Red
        } else if pct > 70.0 {
            Color::Yellow
        } else {
            Color::Green
        };

        let filled_str: String = "█".repeat(filled);
        let empty_str: String = "░".repeat(empty);

        buf.set_string(bar_start, area.y, &filled_str, Style::default().fg(bar_color));
        buf.set_string(
            bar_start + filled as u16,
            area.y,
            &empty_str,
            Style::default().fg(Color::DarkGray),
        );

        // Percentage label
        buf.set_string(bar_end, area.y, &pct_label, Style::default().fg(Color::White));
    }
}
