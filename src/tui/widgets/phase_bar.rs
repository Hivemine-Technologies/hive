use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::domain::Phase;

pub fn render_phase_bar(frame: &mut Frame, area: Rect, current_phase: &Phase) {
    let all_phases = Phase::all_in_order();
    let current_idx = current_phase.pipeline_index();
    let is_terminal = matches!(
        current_phase,
        Phase::Complete | Phase::NeedsAttention { .. }
    );

    let mut spans: Vec<Span> = Vec::new();

    for (i, phase) in all_phases.iter().enumerate() {
        let label = phase_short_label(phase);
        let style = match current_idx {
            Some(ci) if i < ci || (is_terminal && matches!(current_phase, Phase::Complete)) => {
                // Completed phase
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            }
            Some(ci) if i == ci && !is_terminal => {
                // Current active phase
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            }
            _ if matches!(current_phase, Phase::NeedsAttention { .. }) => {
                // After NeedsAttention, show remaining as red
                Style::default().fg(Color::Red)
            }
            _ => {
                // Future phase
                Style::default().fg(Color::DarkGray)
            }
        };

        let icon = match current_idx {
            Some(ci) if i < ci || (is_terminal && matches!(current_phase, Phase::Complete)) => {
                "\u{2713} " // checkmark
            }
            Some(ci) if i == ci && !is_terminal => "\u{25b6} ", // play
            _ => "\u{25cb} ",                                    // circle
        };

        spans.push(Span::styled(format!("{icon}{label}"), style));

        if i < all_phases.len() - 1 {
            spans.push(Span::styled(
                " \u{2192} ",
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    let line = Line::from(spans);
    let bar = Paragraph::new(line);
    frame.render_widget(bar, area);
}

fn phase_short_label(phase: &Phase) -> &'static str {
    match phase {
        Phase::Understand => "Understand",
        Phase::Implement => "Implement",
        Phase::SelfReview { .. } => "SelfReview",
        Phase::CrossReview => "CrossReview",
        Phase::RaisePr => "RaisePR",
        Phase::CiWatch { .. } => "CI",
        Phase::BotReviews { .. } => "BotRev",
        Phase::FollowUps => "FollowUp",
        Phase::Handoff => "Handoff",
        Phase::PrWatch => "PRWatch",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_short_labels() {
        assert_eq!(phase_short_label(&Phase::Understand), "Understand");
        assert_eq!(phase_short_label(&Phase::CiWatch { attempt: 0 }), "CI");
        assert_eq!(phase_short_label(&Phase::BotReviews { cycle: 0 }), "BotRev");
        assert_eq!(phase_short_label(&Phase::Handoff), "Handoff");
    }
}
