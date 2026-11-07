//! The pseudo-locale: accented, lengthened English that shows which text is
//! not translated and where translations will not fit.

/// The language tag that selects the pseudo-locale.
pub const PSEUDO_LOCALE: &str = "en-XA";

/// Accents every ASCII letter and lengthens the text by about 40 percent,
/// inside brackets so a clipped end shows: `Close` becomes `[Çļöšé ~~]`.
///
/// Text between Fluent's isolation marks (U+2068 ... U+2069) is an inserted
/// value, not translatable text, and is kept as it is.
#[must_use]
pub fn pseudolocalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2 + 4);
    out.push('[');
    let mut depth = 0usize;
    let mut letters = 0usize;
    for character in text.chars() {
        match character {
            '\u{2068}' => depth += 1,
            '\u{2069}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && character.is_ascii_alphabetic() {
            letters += 1;
            out.push(accented(character));
        } else {
            out.push(character);
        }
    }
    let padding = (letters * 2).div_ceil(5).max(1);
    out.push(' ');
    out.extend(std::iter::repeat_n('~', padding));
    out.push(']');
    out
}

fn accented(character: char) -> char {
    const LOWER: [char; 26] = [
        'å', 'ƀ', 'ç', 'ð', 'é', 'ƒ', 'ĝ', 'ĥ', 'î', 'ĵ', 'ķ', 'ļ', 'ɱ', 'ñ', 'ö', 'þ', 'ǫ', 'ŕ',
        'š', 'ţ', 'û', 'ṽ', 'ŵ', 'ẋ', 'ý', 'ž',
    ];
    const UPPER: [char; 26] = [
        'Å', 'Ɓ', 'Ç', 'Ð', 'É', 'Ƒ', 'Ĝ', 'Ĥ', 'Î', 'Ĵ', 'Ķ', 'Ļ', 'Ṁ', 'Ñ', 'Ö', 'Þ', 'Ǫ', 'Ŕ',
        'Š', 'Ţ', 'Û', 'Ṽ', 'Ŵ', 'Ẋ', 'Ý', 'Ž',
    ];
    let offset = |base: char| usize::try_from(u32::from(character) - u32::from(base)).ok();
    if character.is_ascii_lowercase() {
        offset('a')
            .and_then(|index| LOWER.get(index))
            .copied()
            .unwrap_or(character)
    } else if character.is_ascii_uppercase() {
        offset('A')
            .and_then(|index| UPPER.get(index))
            .copied()
            .unwrap_or(character)
    } else {
        character
    }
}
