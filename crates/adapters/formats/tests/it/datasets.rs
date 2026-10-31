//! Dataset previews and molecules through the formats adapter.

use std::sync::Arc;

use adapter_formats::Formats;
use application::ports::formats::{DatasetRequest, FormatsPort};
use domain::block::Cell;
use domain::dataset::{DatasetFormat, DatasetSpec};
use parquet::data_type::{ByteArray, ByteArrayType, DoubleType, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;

fn request(path: &str, format: DatasetFormat) -> DatasetRequest {
    DatasetRequest {
        path: path.to_owned(),
        format,
        preview_rows: 2,
        truncated: false,
    }
}

fn preview(bytes: &[u8], request: &DatasetRequest) -> DatasetSpec {
    let spec = Formats
        .preview_dataset(bytes, request)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(spec.validate(), Ok(()));
    spec
}

#[test]
fn csv_columns_are_typed_with_missing_values() {
    let csv = "id,mpg,ok,day,model\n\
               1,21.0,true,2024-03-01,Mazda RX4\n\
               2,NA,false,2024-03-02,\"Datsun, 710\"\n\
               3,22.8,TRUE,2024-03-03,\n\
               4,,false,2024-02-28,Valiant\n";
    let spec = preview(csv.as_bytes(), &request("cars.csv", DatasetFormat::Csv));
    assert_eq!((spec.rows, spec.sampled), (Some(4), false));
    let dtypes: Vec<&str> = spec.columns.iter().map(|c| c.dtype.as_str()).collect();
    assert_eq!(dtypes, ["integer", "float", "bool", "date", "text"]);
    let mpg = &spec.columns[1];
    assert!((mpg.null_percent - 50.0).abs() < 1e-9);
    assert_eq!(mpg.distinct, Some(2));
    assert_eq!(
        (mpg.min.clone(), mpg.max.clone()),
        (Some(Cell::Number(21.0)), Some(Cell::Number(22.8)))
    );
    assert!(mpg.histogram.is_some());
    let day = &spec.columns[3];
    assert_eq!(day.min, Some(Cell::Text("2024-02-28".into())));
    assert_eq!(day.histogram, None);
    assert!((spec.columns[4].null_percent - 25.0).abs() < 1e-9);
    assert_eq!(spec.preview.rows.len(), 2);
    assert_eq!(spec.preview.rows[1][1], Cell::Null);
    assert_eq!(spec.preview.rows[1][4], Cell::Text("Datsun, 710".into()));
    assert_eq!(spec.preview.rows[0][2], Cell::Bool(true));
}

#[test]
fn tsv_and_json_lines_read_too() {
    let tsv = "a\tb\n1\tx\n2\ty\n";
    let spec = preview(tsv.as_bytes(), &request("t.tsv", DatasetFormat::Tsv));
    assert_eq!(spec.columns[0].dtype, "integer");
    let jsonl =
        "{\"n\": 1, \"s\": \"a\", \"tags\": [1]}\n\n{\"n\": 2.5, \"when\": \"2024-01-01T10:00\"}\n";
    let spec = preview(jsonl.as_bytes(), &request("e.jsonl", DatasetFormat::Jsonl));
    assert_eq!(spec.rows, Some(2));
    let names: Vec<&str> = spec.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["n", "s", "tags", "when"]);
    let dtypes: Vec<&str> = spec.columns.iter().map(|c| c.dtype.as_str()).collect();
    assert_eq!(dtypes, ["float", "text", "json", "datetime"]);
    let bad = Formats.preview_dataset(b"[1]\n", &request("e.jsonl", DatasetFormat::Jsonl));
    assert!(bad.is_err());
}

#[test]
fn a_truncated_csv_is_a_sample_without_a_row_count() {
    let mut req = request("big.csv", DatasetFormat::Csv);
    req.truncated = true;
    let spec = preview(b"x\n1\n2\n3", &req);
    assert_eq!((spec.rows, spec.sampled), (None, true));
    assert_eq!(spec.columns[0].distinct, Some(2));
}

fn parquet_fixture() -> Vec<u8> {
    let schema = Arc::new(
        parse_message_type(
            "message cars {
               REQUIRED INT64 id;
               OPTIONAL DOUBLE mpg;
               OPTIONAL BYTE_ARRAY model (UTF8);
             }",
        )
        .unwrap_or_else(|e| panic!("{e}")),
    );
    let mut buffer = Vec::new();
    let props = Arc::new(WriterProperties::builder().build());
    let mut writer =
        SerializedFileWriter::new(&mut buffer, schema, props).unwrap_or_else(|e| panic!("{e}"));
    let mut group = writer.next_row_group().unwrap_or_else(|e| panic!("{e}"));
    let mut ix = 0;
    while let Some(mut column) = group.next_column().unwrap_or_else(|e| panic!("{e}")) {
        let written = match ix {
            0 => column
                .typed::<Int64Type>()
                .write_batch(&[1, 2, 3], None, None),
            1 => column
                .typed::<DoubleType>()
                .write_batch(&[21.0, 30.4], Some(&[1, 0, 1]), None),
            _ => column.typed::<ByteArrayType>().write_batch(
                &[ByteArray::from("Mazda"), ByteArray::from("Fiat")],
                Some(&[1, 1, 0]),
                None,
            ),
        };
        written.unwrap_or_else(|e| panic!("{e}"));
        column.close().unwrap_or_else(|e| panic!("{e}"));
        ix += 1;
    }
    group.close().unwrap_or_else(|e| panic!("{e}"));
    writer.close().unwrap_or_else(|e| panic!("{e}"));
    buffer
}

#[test]
fn parquet_schema_rows_and_statistics() {
    let spec = preview(
        &parquet_fixture(),
        &request("cars.parquet", DatasetFormat::Parquet),
    );
    assert_eq!((spec.rows, spec.sampled), (Some(3), false));
    let dtypes: Vec<&str> = spec.columns.iter().map(|c| c.dtype.as_str()).collect();
    assert_eq!(dtypes, ["int64", "float64", "utf8"]);
    let mpg = &spec.columns[1];
    assert!((mpg.null_percent - 100.0 / 3.0).abs() < 1e-9);
    assert_eq!(mpg.max, Some(Cell::Number(30.4)));
    assert_eq!(spec.columns[2].min, Some(Cell::Text("Fiat".into())));
    assert_eq!(
        spec.preview.rows[0],
        [
            Cell::Number(1.0),
            Cell::Number(21.0),
            Cell::Text("Mazda".into())
        ]
    );
    let bad = Formats.preview_dataset(
        b"not parquet",
        &request("x.parquet", DatasetFormat::Parquet),
    );
    assert!(bad.is_err());
}

#[test]
fn molecules_are_computed_from_smiles() {
    let ethanol = Formats.depict_molecule("CCO", None);
    assert_eq!(ethanol.error, None);
    assert_eq!(ethanol.formula.as_deref(), Some("C2H6O"));
    let mw = ethanol.mw.unwrap_or_default();
    assert!((mw - 46.07).abs() < 0.005, "{mw}");
    assert_eq!(ethanol.charge, Some(0));
    assert!(ethanol.canonical.is_some());
    assert!(ethanol.svg.as_deref().unwrap_or_default().contains("<svg"));
    assert_eq!(ethanol.claimed_matches, None);

    let caffeine = Formats.depict_molecule("Cn1cnc2c1c(=O)n(C)c(=O)n2C", Some("C8H10N4O2"));
    assert_eq!(caffeine.formula.as_deref(), Some("C8H10N4O2"));
    let mw = caffeine.mw.unwrap_or_default();
    assert!((mw - 194.19).abs() < 0.005, "{mw}");
    assert_eq!(caffeine.claimed_matches, Some(true));

    let benzoate = Formats.depict_molecule("O=C([O-])c1ccccc1", None);
    assert_eq!(benzoate.charge, Some(-1));
    assert_eq!(benzoate.formula.as_deref(), Some("C7H5O2"));
}

#[test]
fn bad_smiles_and_wrong_claims_are_reported() {
    let bad = Formats.depict_molecule("C1CC(", None);
    let error = bad.error.unwrap_or_default();
    assert!(error.starts_with("invalid SMILES"), "{error}");
    assert_eq!((bad.svg, bad.formula, bad.mw), (None, None, None));

    let wrong = Formats.depict_molecule("CCO", Some("C2H4O"));
    assert_eq!(wrong.claimed_matches, Some(false));
    assert_eq!(wrong.claimed_formula.as_deref(), Some("C2H4O"));
}
