//! How the engine reads names that mean different things in different
//! places: `ton`, `oz`, `gal`, `hp`, `cal`… numbat picks one meaning
//! silently, so the sheet says which one it used, the way a computer
//! algebra system shows its interpretation.

/// Names with more than one common meaning, and the one numbat uses.
const AMBIGUOUS: &[(&[&str], &str)] = &[
    (
        &["ton", "tons"],
        "`ton` read as the metric tonne (1000 kg); the long ton (2240 lb) is `long_ton`, a \
         short ton is 2000 lb",
    ),
    (
        &["oz", "ounce", "ounces"],
        "`oz` read as the avoirdupois ounce of mass (28.35 g); the troy ounce is `ozt`, the US \
         fluid ounce `floz`",
    ),
    (
        &["gal", "gallon", "gallons"],
        "`gallon` read as the US liquid gallon (3.785 L); the imperial gallon is \
         `imperial_gallon` (4.546 L)",
    ),
    (
        &["pint", "pints"],
        "`pint` read as the US liquid pint (473.2 mL); the imperial pint is `imperial_pint` \
         (568.3 mL)",
    ),
    (
        &["cup", "cups"],
        "`cup` read as the US cup (236.6 mL); a metric cup is 250 mL",
    ),
    (
        &["hp", "horsepower"],
        "`hp` read as metric horsepower (735.5 W); mechanical horsepower is 745.7 W",
    ),
    (
        &["cal", "calorie", "calories"],
        "`cal` read as the small (thermochemical) calorie, 4.184 J; food Calories are `kcal`",
    ),
    (
        &["mi", "mile", "miles"],
        "`mile` read as the statute mile (1609.344 m); the nautical mile is `nmi` (1852 m)",
    ),
    (
        &["lb", "lbs", "pound", "pounds"],
        "`lb` read as the avoirdupois pound of mass (0.45359237 kg); pound-force is `lbf`",
    ),
    (
        &["BTU"],
        "`BTU` read as the International Table BTU (1055.056 J)",
    ),
];

/// The interpretation notes for the names an expression uses, skipping
/// names the sheet defines itself.
pub fn interpretations(names: &[String], defined: &[String]) -> Vec<String> {
    let mut notes = Vec::new();
    for name in names {
        if defined.contains(name) {
            continue;
        }
        if let Some((_, note)) = AMBIGUOUS
            .iter()
            .find(|(aliases, _)| aliases.contains(&name.as_str()))
        {
            let note = (*note).to_owned();
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_each_ambiguous_name_once() {
        let names: Vec<String> = ["ton", "tons", "kg", "oz"].map(String::from).to_vec();
        let notes = interpretations(&names, &[]);
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("metric tonne"));
        assert!(notes[1].contains("avoirdupois"));
        assert!(interpretations(&["mi".into()], &["mi".into()]).is_empty());
    }
}
