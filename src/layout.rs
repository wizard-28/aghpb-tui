use ratatui::{layout::Flex, prelude::*};

pub fn centered_rect(area: Rect, horizontal: Constraint, vertical: Constraint) -> Rect {
    let [area] = Layout::horizontal([horizontal])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([vertical]).flex(Flex::Center).areas(area);
    area
}

pub fn centered_text<const S: usize>(text: [&str; S], height: u16) -> String {
    let num_of_lines = text.len();
    let text = text.join("\n");
    format!(
        "{}{text}",
        // HACK: Get the text centered vertically
        "\n".repeat(
            (f32::from(height) / 2f32) as usize - usize::from(height % 2 == 0) - num_of_lines
        ),
    )
}
