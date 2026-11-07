use super::{
    design::*,
    window::{Command, Message},
};
use crate::bluetooth::aacp::{BatteryComponent, BatteryInfo, BatteryStatus};
use crate::devices::enums::{AirPodsNoiseControlMode as Mode, AirPodsState};
use iced::widget::{column, progress_bar, row, text};
use iced::{Center, Element, Length};

pub fn battery_label(battery: Option<&BatteryInfo>) -> String {
    battery
        .filter(|b| b.status != BatteryStatus::Disconnected && b.level <= 100)
        .map(|b| format!("{}%", b.level))
        .unwrap_or_else(|| "—".into())
}
pub fn battery_view(batteries: &[BatteryInfo]) -> Element<'_, Message> {
    let components = if batteries
        .iter()
        .any(|b| b.component == BatteryComponent::Headphone)
    {
        vec![(BatteryComponent::Headphone, "Headphones")]
    } else {
        vec![
            (BatteryComponent::Left, "Left"),
            (BatteryComponent::Right, "Right"),
            (BatteryComponent::Case, "Case"),
        ]
    };
    let mut cards = row![].spacing(8);
    for (component, label) in components {
        let battery = batteries.iter().find(|b| b.component == component);
        let known = battery.filter(|b| b.status != BatteryStatus::Disconnected && b.level <= 100);
        let mut content = column![muted(label), text(battery_label(battery)).size(23)]
            .spacing(8)
            .align_x(Center);
        if let Some(b) = known {
            content = content.push(progress_bar(0.0..=100.0, b.level as f32).girth(3));
        }
        content = content.push(muted(match known {
            Some(b) if b.status == BatteryStatus::Charging => "Charging",
            Some(_) => "Battery",
            None => "Unavailable",
        }));
        cards = cards.push(card(content).width(Length::Fill));
    }
    cards.into()
}
pub fn controls<'a>(id: &'a str, state: &AirPodsState, busy: bool) -> Element<'a, Message> {
    let mode = |value: Mode| {
        let selected = state.noise_control_mode.to_byte() == value.to_byte();
        action(
            value.to_string(),
            (!busy).then(|| Message::Run(id.into(), Command::Mode(value))),
            selected,
        )
    };
    let mut second = row![mode(Mode::Adaptive)].spacing(8);
    if state.allow_off_mode {
        second = second.push(mode(Mode::Off));
    }
    column![
        text("Listening Mode").size(15),
        row![mode(Mode::NoiseCancellation), mode(Mode::Transparency)].spacing(8),
        second,
        setting(
            "Conversation Awareness",
            "Lower audio volume when you speak.",
            state.conversation_awareness_enabled,
            !busy,
            move |v| Message::Run(id.into(), Command::Conversation(v))
        )
    ]
    .spacing(8)
    .into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_battery_is_not_zero() {
        assert_eq!(battery_label(None), "—");
    }
    #[test]
    fn disconnected_and_invalid_battery_are_unknown() {
        for (status, level) in [
            (BatteryStatus::Disconnected, 50),
            (BatteryStatus::NotCharging, 255),
        ] {
            assert_eq!(
                battery_label(Some(&BatteryInfo {
                    component: BatteryComponent::Left,
                    level,
                    status
                })),
                "—"
            );
        }
    }
    #[test]
    fn real_zero_is_visible() {
        assert_eq!(
            battery_label(Some(&BatteryInfo {
                component: BatteryComponent::Case,
                level: 0,
                status: BatteryStatus::NotCharging
            })),
            "0%"
        );
    }
}
