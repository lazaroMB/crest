//! Science viewers on fixture files: a GenBank plasmid and a FASTA file
//! (both generated for these tests).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use adapter_formats::Formats;
use application::ports::formats::{FormatsPort, SequenceRequest};
use domain::science::sequence::{Alphabet, SequenceFormat, Strand, Topology};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/science/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn request(path: &str) -> SequenceRequest {
    SequenceRequest {
        path: Some(path.into()),
        record: 0,
        alphabet: None,
        topology: None,
        enzymes: true,
    }
}

#[test]
fn genbank_plasmid_with_features_and_sites() {
    let spec = Formats::new()
        .parse_sequence(&fixture("pcrest1.gb"), &request("dna/pcrest1.gb"))
        .unwrap();
    spec.validate().unwrap();
    assert_eq!(spec.format, SequenceFormat::Genbank);
    assert_eq!(spec.name.as_deref(), Some("pCrest1"));
    assert_eq!(spec.alphabet, Alphabet::Dna);
    assert_eq!(spec.topology, Topology::Circular);
    assert_eq!(spec.len(), 515);
    assert_eq!(spec.accession, None);
    assert_eq!(spec.organism.as_deref(), Some("synthetic DNA construct"));
    // `source` is not drawn.
    let kinds: Vec<&str> = spec.features.iter().map(|f| f.kind.as_str()).collect();
    assert_eq!(kinds, ["promoter", "CDS", "terminator", "misc_feature"]);
    let cds = &spec.features[1];
    assert_eq!(cds.name(), "crsA");
    assert_eq!(cds.location_text(), "118..213");
    assert_eq!(cds.qualifier("product"), Some("test protein"));
    assert!(spec.cds_issues().is_empty(), "{:?}", spec.cds_issues());
    let translated = domain::science::sequence::translate(spec.feature_sequence(cds).as_bytes());
    assert_eq!(translated, "MPFSRGNTSETPANFEIKTRRTITDRNSATF*");
    let terminator = &spec.features[2];
    assert_eq!(terminator.strand, Strand::Reverse);
    assert_eq!(terminator.location_text(), "complement(265..299)");
    // join(476..515,1..20) crosses the origin: two segments.
    let ori = &spec.features[3];
    assert_eq!(ori.segments.len(), 2);
    assert_eq!(ori.bounds(), Some((475, 20)));
    assert_eq!(ori.len(), 60);
    // The three sites the generator placed, each cut once.
    for (enzyme, cut) in [("EcoRI", 41), ("BamHI", 234), ("HindIII", 360)] {
        let sites: Vec<_> = spec.cut_sites.iter().filter(|s| s.enzyme == enzyme).collect();
        assert_eq!(sites.len(), 1, "{enzyme}");
        assert_eq!(sites[0].cut, cut, "{enzyme}");
    }
    assert!(spec.unique_cutters().contains(&"EcoRI"));
    assert!(spec.enzyme_source.as_deref().unwrap().contains("REBASE"));
    let gc = spec.gc().unwrap();
    assert!((30.0..70.0).contains(&gc), "{gc}");
}

#[test]
fn fasta_records_and_inline_residues() {
    let formats = Formats::new();
    let protein = formats
        .parse_sequence(&fixture("crsa.fasta"), &request("prot/crsa.fasta"))
        .unwrap();
    assert_eq!(protein.format, SequenceFormat::Fasta);
    assert_eq!(protein.alphabet, Alphabet::Protein);
    assert_eq!(protein.name.as_deref(), Some("crsA"));
    assert_eq!(protein.description.as_deref(), Some("test protein of pCrest1"));
    assert_eq!(protein.records, 2);
    assert_eq!(protein.sequence, "MPFSRGNTSETPANFEIKTRRTITDRNSATF");
    assert!(protein.cut_sites.is_empty());
    assert!(protein.warnings[0].contains("2 records"), "{:?}", protein.warnings);
    let second = formats
        .parse_sequence(
            &fixture("crsa.fasta"),
            &SequenceRequest {
                record: 1,
                ..request("prot/crsa.fasta")
            },
        )
        .unwrap();
    assert_eq!(second.sequence, "MKV");
    let missing = formats.parse_sequence(
        &fixture("crsa.fasta"),
        &SequenceRequest {
            record: 2,
            ..request("prot/crsa.fasta")
        },
    );
    assert!(missing.unwrap_err().to_string().contains("does not exist"));

    let inline = formats
        .parse_sequence(
            b"  1 acgtga attcga\n 13 uuu",
            &SequenceRequest {
                path: None,
                ..request("x")
            },
        )
        .unwrap();
    assert_eq!(inline.format, SequenceFormat::Plain);
    assert_eq!(inline.sequence, "ACGTGAATTCGAUUU");
    // Both T and U: DNA, with the U counted as a valid code.
    assert_eq!(inline.alphabet, Alphabet::Dna);
    assert_eq!(inline.cut_sites.len(), 1);
    assert!(formats.parse_sequence(b"   \n", &request("x")).is_err());
}

#[test]
fn reaction_balance_suggestions_and_errors() {
    use domain::computed::ComputedStatus;
    use domain::science::reaction::{ReactionRole, Stoichiometry};
    let formats = Formats::new();
    // Ethanol to acetic acid: balanced as written.
    let ox = formats.depict_reaction("CCO.O=O>>CC(=O)O.O", None);
    assert_eq!(ox.error, None);
    ox.validate().unwrap();
    assert!(ox.is_balanced());
    assert_eq!(ox.stoichiometry, Stoichiometry::Balanced);
    assert_eq!(ox.balance.value("balanced").unwrap().v, 1.0);
    assert_eq!(ox.balance.status, ComputedStatus::Ok);
    assert_eq!(ox.equation(None), "C2H6O + O2 → C2H4O2 + H2O");
    assert!(ox.components[0].molecule.svg.is_some());

    // Methane combustion needs 1, 2, 1, 2.
    let burn = formats.depict_reaction("C.O=O>>O=C=O.O", None);
    assert!(!burn.is_balanced());
    assert_eq!(
        burn.stoichiometry,
        Stoichiometry::Suggested {
            coefficients: vec![1, 2, 1, 2]
        }
    );
    assert_eq!(burn.balance.value("delta.O").unwrap().v, 1.0);
    assert_eq!(burn.balance.value("suggested.2").unwrap().v, 2.0);
    assert_eq!(burn.balance.status, ComputedStatus::Warning);
    assert_eq!(
        burn.balance.method.params["suggested"],
        "CH4 + 2 O2 → CO2 + 2 H2O"
    );
    // With those coefficients it balances.
    let given = formats.depict_reaction("C.O=O>>O=C=O.O", Some(&[1, 2, 1, 2]));
    assert!(given.is_balanced());
    assert!(formats
        .depict_reaction("C.O=O>>O=C=O.O", Some(&[1, 2]))
        .error
        .unwrap()
        .contains("2 coefficients given for 4"));

    // Fe2+ to Fe3+ balances in iron but never in charge.
    let fe = formats.depict_reaction("[Fe+2]>>[Fe+3]", None);
    assert_eq!(fe.charge, (2, 3));
    assert_eq!(fe.stoichiometry, Stoichiometry::Impossible);
    assert!(fe.balance.warnings.iter().any(|w| w.code == "cannot_balance"));

    // Agents are drawn but not counted.
    let hyd = formats.depict_reaction("CC=C.[H][H]>[Pd]>CCC", None);
    assert!(hyd.is_balanced());
    assert_eq!(hyd.side(ReactionRole::Agent).count(), 1);

    let bad = formats.depict_reaction("C1CC>>C", None);
    assert!(bad.error.is_some());
    assert_eq!(bad.balance.status, ComputedStatus::Error);
    assert!(formats.depict_reaction("CCO", None).error.unwrap().contains("not a reaction"));
}
