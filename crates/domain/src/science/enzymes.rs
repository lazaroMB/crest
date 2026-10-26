//! A small table of common restriction enzymes and where they cut a
//! sequence, for the sequence viewer's sites and plasmid map.
//!
//! The recognition sequences and cut positions are those REBASE lists
//! ([`ENZYME_SOURCE`]); only enzymes with an unambiguous site are here
//! (no `N` spacers). A site matches A, C, G and T exactly: an ambiguous
//! base of the sequence never makes a site.

use super::sequence::{CutSite, Strand};

/// Where the table comes from.
pub const ENZYME_SOURCE: &str = "REBASE (rebase.neb.com): Roberts RJ, Vincze T, Posfai J, \
                                 Macelis D. Nucleic Acids Res. 2023;51(D1):D629-D630";

/// A restriction enzyme: its site on the top strand and its cuts, as
/// offsets from the site's first base (`top` on the top strand, `bottom`
/// on the bottom strand in top-strand coordinates; EcoRI G^AATTC is
/// `top` 1, `bottom` 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enzyme {
    /// Name.
    pub name: &'static str,
    /// Recognition sequence, 5'→3' on the top strand.
    pub site: &'static str,
    /// Top-strand cut, before base `top` of the site.
    pub top: i32,
    /// Bottom-strand cut, in top-strand coordinates.
    pub bottom: i32,
}

const fn palindrome(name: &'static str, site: &'static str, top: i32) -> Enzyme {
    Enzyme {
        name,
        site,
        top,
        // A palindromic site cuts the bottom strand symmetrically.
        bottom: site.len() as i32 - top,
    }
}

/// The bundled enzymes, by name.
pub const ENZYMES: &[Enzyme] = &[
    palindrome("AatII", "GACGTC", 5),
    palindrome("AflII", "CTTAAG", 1),
    palindrome("AgeI", "ACCGGT", 1),
    palindrome("ApaI", "GGGCCC", 5),
    palindrome("AscI", "GGCGCGCC", 2),
    palindrome("AvrII", "CCTAGG", 1),
    palindrome("BamHI", "GGATCC", 1),
    Enzyme {
        name: "BbsI",
        site: "GAAGAC",
        top: 8,
        bottom: 12,
    },
    palindrome("BglII", "AGATCT", 1),
    Enzyme {
        name: "BsaI",
        site: "GGTCTC",
        top: 7,
        bottom: 11,
    },
    Enzyme {
        name: "BsmBI",
        site: "CGTCTC",
        top: 7,
        bottom: 11,
    },
    palindrome("BspHI", "TCATGA", 1),
    palindrome("BsrGI", "TGTACA", 1),
    palindrome("ClaI", "ATCGAT", 2),
    palindrome("DraI", "TTTAAA", 3),
    palindrome("EcoRI", "GAATTC", 1),
    palindrome("EcoRV", "GATATC", 3),
    palindrome("FseI", "GGCCGGCC", 6),
    palindrome("HindIII", "AAGCTT", 1),
    palindrome("HpaI", "GTTAAC", 3),
    palindrome("KpnI", "GGTACC", 5),
    palindrome("MfeI", "CAATTG", 1),
    palindrome("MluI", "ACGCGT", 1),
    palindrome("NcoI", "CCATGG", 1),
    palindrome("NdeI", "CATATG", 2),
    palindrome("NheI", "GCTAGC", 1),
    palindrome("NotI", "GCGGCCGC", 2),
    palindrome("NsiI", "ATGCAT", 5),
    palindrome("PacI", "TTAATTAA", 5),
    palindrome("PciI", "ACATGT", 1),
    palindrome("PmeI", "GTTTAAAC", 4),
    palindrome("PstI", "CTGCAG", 5),
    palindrome("PvuI", "CGATCG", 4),
    palindrome("PvuII", "CAGCTG", 3),
    palindrome("SacI", "GAGCTC", 5),
    palindrome("SacII", "CCGCGG", 4),
    palindrome("SalI", "GTCGAC", 1),
    Enzyme {
        name: "SapI",
        site: "GCTCTTC",
        top: 8,
        bottom: 11,
    },
    palindrome("SbfI", "CCTGCAGG", 6),
    palindrome("ScaI", "AGTACT", 3),
    palindrome("SmaI", "CCCGGG", 3),
    palindrome("SnaBI", "TACGTA", 3),
    palindrome("SpeI", "ACTAGT", 1),
    palindrome("SphI", "GCATGC", 5),
    palindrome("StuI", "AGGCCT", 3),
    palindrome("SwaI", "ATTTAAAT", 4),
    palindrome("XbaI", "TCTAGA", 1),
    palindrome("XhoI", "CTCGAG", 1),
    palindrome("XmaI", "CCCGGG", 1),
];

fn revcomp(site: &str) -> Vec<u8> {
    site.bytes()
        .rev()
        .map(|b| match b {
            b'A' => b'T',
            b'T' => b'A',
            b'C' => b'G',
            b'G' => b'C',
            other => other,
        })
        .collect()
}

/// Where each enzyme of `enzymes` cuts `seq` (upper or lower case, T or
/// U), on both strands for non-palindromic sites and across the origin
/// when `circular`. On a linear sequence a site whose cut falls outside
/// it is left out. Sorted by cut, then enzyme.
#[must_use]
pub fn find_cut_sites(seq: &[u8], circular: bool, enzymes: &[Enzyme]) -> Vec<CutSite> {
    let len = seq.len();
    let upper: Vec<u8> = seq
        .iter()
        .map(|b| match b.to_ascii_uppercase() {
            b'U' => b'T',
            other => other,
        })
        .collect();
    let mut sites = Vec::new();
    let Ok(len_i) = i64::try_from(len) else {
        return sites;
    };
    for enzyme in enzymes {
        let forward = enzyme.site.as_bytes().to_vec();
        let reverse = revcomp(enzyme.site);
        let k = forward.len();
        if k == 0 || k > len {
            continue;
        }
        let mut haystack = upper.clone();
        if circular {
            haystack.extend_from_slice(&upper[..k - 1]);
        }
        let mut strands = vec![(forward.clone(), Strand::Forward)];
        if reverse != forward {
            strands.push((reverse, Strand::Reverse));
        }
        for (pattern, strand) in strands {
            for start in 0..=(haystack.len() - k) {
                if start >= len {
                    break;
                }
                if haystack[start..start + k] != pattern[..] {
                    continue;
                }
                let start_i = i64::try_from(start).unwrap_or(0);
                let k_i = i64::try_from(k).unwrap_or(0);
                let cut = match strand {
                    Strand::Reverse => start_i + k_i - i64::from(enzyme.bottom),
                    _ => start_i + i64::from(enzyme.top),
                };
                let cut = if circular {
                    cut.rem_euclid(len_i)
                } else if (0..=len_i).contains(&cut) {
                    cut
                } else {
                    continue;
                };
                sites.push(CutSite {
                    enzyme: enzyme.name.to_owned(),
                    site: enzyme.site.to_owned(),
                    start,
                    cut: usize::try_from(cut).unwrap_or(0),
                    strand,
                });
            }
        }
    }
    sites.sort_by(|a, b| a.cut.cmp(&b.cut).then_with(|| a.enzyme.cmp(&b.enzyme)));
    sites
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_sane() {
        let names: Vec<&str> = ENZYMES.iter().map(|e| e.name).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| n.to_ascii_lowercase());
        assert_eq!(names, sorted);
        for e in ENZYMES {
            assert!(e.site.bytes().all(|b| b"ACGT".contains(&b)), "{}", e.name);
        }
        let eco = ENZYMES.iter().find(|e| e.name == "EcoRI").unwrap();
        assert_eq!((eco.top, eco.bottom), (1, 5));
    }

    #[test]
    fn sites_on_both_strands_and_across_the_origin() {
        // EcoRI at 2: cut after G, before base 3.
        let sites = find_cut_sites(b"AAGAATTCAA", false, ENZYMES);
        assert_eq!(sites.len(), 1);
        assert_eq!((sites[0].enzyme.as_str(), sites[0].start, sites[0].cut), ("EcoRI", 2, 3));
        // BsaI on the reverse strand (GAGACC) cuts 5 bases before it.
        let rev = find_cut_sites(b"AAAAAAAAGAGACCAA", false, ENZYMES);
        let bsa = rev.iter().find(|s| s.enzyme == "BsaI").unwrap();
        assert_eq!((bsa.start, bsa.cut, bsa.strand), (8, 3, Strand::Reverse));
        // A circular sequence with GAATTC split across its ends.
        let wrap = find_cut_sites(b"TTCAAAAGAA", true, ENZYMES);
        let eco = wrap.iter().find(|s| s.enzyme == "EcoRI").unwrap();
        assert_eq!((eco.start, eco.cut), (7, 8));
        assert!(find_cut_sites(b"TTCAAAAGAA", false, ENZYMES).is_empty());
    }
}
