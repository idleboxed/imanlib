// SPDX-License-Identifier: BSD-3-Clause
use super::{Probe, Stage, END_MS};
use embedded_graphics::{
    mono_font::{iso_8859_5::FONT_10X20, MonoTextStyle},
    pixelcolor::Rgb888,
    prelude::*,
    primitives::Rectangle,
    text::{Baseline, Text},
};
pub fn draw<D: DrawTarget<Color = Rgb888>>(
    probe: &Probe,
    ms: u64,
    target: &mut D,
) -> Result<(), D::Error> {
    target.clear(Rgb888::new(15, 21, 29))?;
    let text = |target: &mut D, value: &str, x, y, color| {
        Text::with_baseline(
            value,
            Point::new(x, y),
            MonoTextStyle::new(&FONT_10X20, color),
            Baseline::Top,
        )
        .draw(target)
        .map(|_| ())
    };
    let white = Rgb888::new(230, 234, 231);
    let green = Rgb888::new(119, 207, 180);
    text(target, "IGUI: проверка ввода evdev", 20, 12, green)?;
    text(
        target,
        &format!(
            "Потоки: {}/8; одновременно: {}. Защитное завершение через {} с.",
            probe.streams.iter().flatten().count(),
            probe.max_overlap,
            END_MS.saturating_sub(ms).div_ceil(1000)
        ),
        20,
        38,
        white,
    )?;
    let prompt = match probe.stage {
        Stage::Prepare => "На первом геймпаде дважды нажмите и отпустите START. Не спешите.".into(),
        Stage::Player { player } => {
            format!("ГЕЙМПАД {player}: нажмите любую кнопку, кроме START первого геймпада.")
        }
        Stage::Button { player, action } => {
            let label = match action {
                "Up" => "ВВЕРХ",
                "Down" => "ВНИЗ",
                "Left" => "ВЛЕВО",
                "Right" => "ВПРАВО",
                "X" => "X (слева)",
                "B" => "B (справа)",
                "Y" => "Y (сверху)",
                "A" => "A (снизу)",
                _ => "Select",
            };
            format!(
                "ГЕЙМПАД {player}: нажмите и отпустите {label}, затем отдельно START на первом."
            )
        }
        Stage::Together => {
            "На всех геймпадах держите X одновременно, отпустите, повторите. START - закончить."
                .into()
        }
        Stage::Finish => {
            "Проверка окончена. Ждите экспорта отчёта и возврата в штатное меню.".into()
        }
    };
    text(target, &prompt, 20, 70, green)?;
    let hint = match probe.stage {
        Stage::Prepare => "START будет управлять только тестом; его код заранее не предполагается.",
        Stage::Finish => "Не отключайте питание, пока не появилось штатное меню.",
        _ => {
            "START первого геймпада: подтвердить / без нажатий пропустить; держать 2 с - закончить."
        }
    };
    text(target, hint, 20, 96, white)?;
    let observation = if !probe.guide.notice.is_empty() {
        probe.guide.notice.to_string()
    } else if probe.guide.ambiguous {
        "Несколько кнопок. START очистит шаг для повторного нажатия.".into()
    } else if let Some(k) = probe.guide.candidate {
        format!(
            "Получено KEY {} из потока #{}. Проверьте подсказку и подтвердите START.",
            k.code, k.generation
        )
    } else {
        "Шаг сам не сменится. Без события START пропускает шаг или отсутствующий геймпад.".into()
    };
    text(target, &observation, 20, 122, white)?;
    for (slot, stream) in probe.streams.iter().enumerate() {
        let x = 20 + (slot % 2) as i32 * 630;
        let y = 156 + (slot / 2) as i32 * 126;
        target.fill_solid(
            &Rectangle::new(Point::new(x, y), Size::new(610, 114)),
            Rgb888::new(23, 32, 42),
        )?;
        if let Some(s) = stream {
            text(
                target,
                &format!("Поток {} / #{}: {}", slot + 1, s.generation, s.info.path),
                x + 10,
                y + 5,
                green,
            )?;
            let name: String = s
                .info
                .name
                .chars()
                .filter(|c| !c.is_control())
                .take(56)
                .collect();
            text(target, &name, x + 10, y + 29, white)?;
            text(
                target,
                &format!(
                    "KEY: {}  ABS: {}  события: {}  нажато: {}",
                    s.info.key_codes.len(),
                    s.info.axis_codes.len(),
                    s.events,
                    s.buttons_down().len()
                ),
                x + 10,
                y + 53,
                white,
            )?;
            let last = s
                .last
                .map(|e| {
                    format!(
                        "{} {} = {}",
                        if e.kind == 1 { "KEY" } else { "ABS" },
                        e.code,
                        e.value
                    )
                })
                .unwrap_or_else(|| "Нет событий".into());
            text(target, &last, x + 10, y + 77, white)?;
        } else {
            text(
                target,
                &format!("Поток {}: нет подходящего устройства", slot + 1),
                x + 10,
                y + 5,
                white,
            )?;
        }
    }
    text(target,"USB-профили evdev. Чтение без grab. Раскладка не сохраняется. Поток не равен игроку.",20,680,white)
}
