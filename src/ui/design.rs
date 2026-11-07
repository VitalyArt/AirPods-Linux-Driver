//! Shared visual vocabulary for the compact device panel.
use super::window::Message;
use iced::widget::{Space, button, column, container, row, text, toggler};
use iced::{Background, Border, Color, Element, Length, Theme};

pub fn muted<'a>(label: impl Into<String>) -> iced::widget::Text<'a> {
    text(label.into())
        .size(12)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(
                theme
                    .extended_palette()
                    .background
                    .base
                    .text
                    .scale_alpha(0.72),
            ),
        })
}
pub fn card<'a>(content: impl Into<Element<'a, Message>>) -> iced::widget::Container<'a, Message> {
    container(content)
        .padding(16)
        .width(Length::Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(if theme.palette().background.r < 0.2 {
                Color::from_rgb8(35, 37, 42)
            } else {
                Color::WHITE
            })),
            border: Border {
                radius: 16.into(),
                ..Default::default()
            },
            ..Default::default()
        })
}
pub fn action<'a>(
    label: impl Into<String>,
    message: Option<Message>,
    selected: bool,
) -> Element<'a, Message> {
    let content = button(text(label.into()).size(13).center())
        .on_press_maybe(message.clone())
        .width(Length::Fill)
        .padding([14, 8])
        .style(move |theme: &Theme, status| {
            let palette = theme.palette();
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let disabled = matches!(status, button::Status::Disabled);
            button::Style {
                background: Some(Background::Color(if disabled {
                    if selected {
                        palette.primary.scale_alpha(0.12)
                    } else {
                        palette.text.scale_alpha(0.04)
                    }
                } else if selected {
                    palette.primary
                } else {
                    palette.text.scale_alpha(if hovered { 0.12 } else { 0.055 })
                })),
                text_color: if disabled {
                    palette.text.scale_alpha(0.45)
                } else if selected {
                    Color::WHITE
                } else {
                    palette.text
                },
                border: Border {
                    radius: 12.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        });
    super::focus::control(content, message)
}
pub fn setting<'a>(
    title: &'a str,
    description: &'a str,
    value: bool,
    enabled: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> Element<'a, Message> {
    let keyboard_action = enabled.then(|| on_toggle(!value));
    let toggle = toggler(value).size(22).style(|theme: &Theme, status| {
        let mut style = toggler::default(theme, status);
        let (on, disabled) = match status {
            toggler::Status::Active { is_toggled } | toggler::Status::Hovered { is_toggled } => {
                (is_toggled, false)
            }
            toggler::Status::Disabled { is_toggled } => (is_toggled, true),
        };
        style.background = Background::Color(
            if on {
                theme.palette().primary
            } else {
                theme.palette().text.scale_alpha(0.22)
            }
            .scale_alpha(if disabled { 0.5 } else { 1.0 }),
        );
        style.foreground =
            Background::Color(Color::WHITE.scale_alpha(if disabled { 0.6 } else { 1.0 }));
        style
    });
    let toggle = if enabled {
        toggle.on_toggle(on_toggle)
    } else {
        toggle
    };
    super::focus::control(
        card(
            row![
                column![text(title).size(14), muted(description)]
                    .spacing(5)
                    .width(Length::Fill),
                Space::new().width(8),
                toggle
            ]
            .align_y(iced::Center),
        ),
        keyboard_action,
    )
}
pub fn headphones<'a>() -> iced::widget::Svg<'a> {
    iced::widget::svg(iced::widget::svg::Handle::from_memory(
        include_bytes!("../../assets/ui/headphones.svg").as_slice(),
    ))
    .width(144)
    .height(72)
}

pub fn link<'a>(label: impl Into<String>, message: Message) -> Element<'a, Message> {
    super::focus::control(
        button(text(label.into()).size(13))
            .padding(10)
            .style(button::text)
            .on_press(message.clone()),
        Some(message),
    )
}

pub fn input_style(
    theme: &Theme,
    status: iced::widget::text_input::Status,
) -> iced::widget::text_input::Style {
    use iced::widget::text_input;
    let mut style = text_input::default(theme, status);
    let focused = matches!(status, text_input::Status::Focused { .. });
    style.background = Background::Color(theme.palette().background);
    style.border = Border {
        color: if focused {
            theme.palette().primary
        } else {
            theme.palette().text.scale_alpha(0.15)
        },
        width: if focused { 2.0 } else { 1.0 },
        radius: 10.into(),
    };
    style
}
