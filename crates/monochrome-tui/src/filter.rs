pub fn fold(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for character in text.chars() {
        for lowered in character.to_lowercase() {
            match lowered {
                '\u{0300}'..='\u{036f}' => {}
                'ı' => folded.push('i'),
                'ø' => folded.push('o'),
                'ł' => folded.push('l'),
                'đ' | 'ð' => folded.push('d'),
                'ß' => folded.push_str("ss"),
                'æ' => folded.push_str("ae"),
                'œ' => folded.push_str("oe"),
                'þ' => folded.push_str("th"),
                other => folded.push(strip_accent(other)),
            }
        }
    }
    folded
}

pub fn matches(haystack: &str, folded_needle: &str) -> bool {
    folded_needle.is_empty() || fold(haystack).contains(folded_needle)
}

fn strip_accent(character: char) -> char {
    match character {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' | 'ŧ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_only_lowercased() {
        assert_eq!(fold("Daft Punk"), "daft punk");
    }

    #[test]
    fn a_dotted_capital_i_folds_to_a_plain_i() {
        assert_eq!(fold("İnsan İnsan"), "insan insan");
        assert!(matches("İnsan İnsan", &fold("insan")));
    }

    #[test]
    fn a_dotless_i_and_a_dotted_one_are_the_same_letter() {
        assert_eq!(fold("Işıklar"), "isiklar");
        assert!(matches("Mavi Işıklar", &fold("ışık")));
        assert!(matches("Mavi Işıklar", &fold("isik")));
    }

    #[test]
    fn turkish_letters_match_their_plain_spelling() {
        assert!(matches("Aşkımız Bitti", &fold("askimiz")));
        assert!(matches("Çav Bella", &fold("cav")));
        assert!(matches("Değirmenler", &fold("degirmenler")));
        assert!(matches("Güllerin İçinden", &fold("gullerin icinden")));
    }

    #[test]
    fn accents_from_other_languages_fold_too() {
        assert!(matches("Beyoncé", &fold("beyonce")));
        assert!(matches("Über", &fold("uber")));
        assert!(matches("Sigur Rós", &fold("sigur ros")));
        assert!(matches("Mötley Crüe", &fold("motley crue")));
        assert!(matches("Björk", &fold("bjork")));
    }

    #[test]
    fn letters_written_as_a_base_plus_a_combining_mark_fold_the_same_way() {
        let decomposed = "Beyonce\u{0301}";
        assert_eq!(fold(decomposed), "beyonce");
        assert!(matches(decomposed, &fold("beyoncé")));
    }

    #[test]
    fn ligatures_and_sharp_s_expand() {
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("Æon"), "aeon");
    }

    #[test]
    fn an_empty_needle_matches_everything() {
        assert!(matches("anything at all", ""));
    }

    #[test]
    fn a_needle_that_is_not_there_does_not_match() {
        assert!(!matches("Daft Punk", &fold("zzz")));
    }

    #[test]
    fn folding_leaves_scripts_without_case_alone() {
        assert_eq!(fold("東京"), "東京");
        assert!(matches("東京は夜の七時", &fold("東京")));
    }
}
