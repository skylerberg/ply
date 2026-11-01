use ply_core::codec::{FORMAT, Reader, Writer, fun, tag};
use ply_core::float::bits;
use ply_core::{Decimal, Fixed, IntegerType};

fn read(bytes: &[u8]) -> Reader<&[u8]> {
    Reader::new(bytes).expect("bytes that open with the format")
}

fn written(write: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    write(&mut w);
    w.finish()
}

fn with(body: &[u8]) -> Vec<u8> {
    [&FORMAT[..], body].concat()
}

#[test]
fn every_kind_of_value_the_codec_writes_is_read_back_unchanged() {
    let ints = [0, 1, -1, 63, -64, 64, i64::MIN, i64::MAX];
    let decimal = Decimal::new(-12345, 3);
    let fixed = [
        Fixed::new(IntegerType::U8, 255),
        Fixed::new(IntegerType::I8, -5i128 as u128),
        Fixed::new(IntegerType::I64, i128::from(i64::MIN) as u128),
        Fixed::new(IntegerType::U64, u128::from(u64::MAX)),
        Fixed::new(IntegerType::I128, i128::MIN as u128),
        Fixed::new(IntegerType::U128, u128::MAX),
    ];
    let bytes = written(|w| {
        w.count(tag::LIST, 3);
        w.tag(tag::UNIT);
        w.tag(tag::TRUE);
        w.tag(tag::FALSE);
        for n in ints {
            w.int(n);
        }
        w.float(bits(-0.5));
        w.decimal(decimal.serialize());
        for f in fixed {
            w.fixed(f);
        }
        w.char(u32::from('é'));
        w.str("né".as_bytes());
        w.bytes(&[0, 255]);
        w.ctor(b"Some", 1);
        w.count(tag::RECORD, 1);
        w.name(b"field");
        w.handle(3);
        w.fun(fun::PROJECT);
        w.varint(2);
        w.varint(1);
        w.again(4);
    });

    let mut r = read(&bytes);
    r.expect(tag::LIST).unwrap();
    assert_eq!(r.count().unwrap(), 3);
    for t in [tag::UNIT, tag::TRUE, tag::FALSE] {
        r.expect(t).unwrap();
    }
    for n in ints {
        r.expect(tag::INT).unwrap();
        assert_eq!(r.int().unwrap(), n);
    }
    r.expect(tag::FLOAT).unwrap();
    assert_eq!(u64::from_le_bytes(r.array().unwrap()), bits(-0.5));
    r.expect(tag::DECIMAL).unwrap();
    assert_eq!(Decimal::deserialize(r.array().unwrap()), decimal);
    for f in fixed {
        r.expect(tag::FIXED).unwrap();
        assert_eq!(r.fixed().unwrap(), f);
    }
    r.expect(tag::CHAR).unwrap();
    assert_eq!(r.char().unwrap(), 'é');
    r.expect(tag::STR).unwrap();
    assert_eq!(r.str().unwrap(), "né");
    r.expect(tag::BYTES).unwrap();
    assert_eq!(r.blob().unwrap(), [0, 255]);
    r.expect(tag::CTOR).unwrap();
    assert_eq!(r.symbol().unwrap().as_str(), "Some");
    assert_eq!(r.varint().unwrap(), 1);
    r.expect(tag::RECORD).unwrap();
    assert_eq!(r.count().unwrap(), 1);
    assert_eq!(r.symbol().unwrap().as_str(), "field");
    r.expect(tag::HANDLE).unwrap();
    assert_eq!(r.varint().unwrap(), 3);
    r.expect(tag::FUN).unwrap();
    assert_eq!(r.byte().unwrap(), fun::PROJECT);
    assert_eq!((r.varint().unwrap(), r.varint().unwrap()), (2, 1));
    r.expect(tag::BACK_REFERENCE).unwrap();
    assert_eq!(r.varint().unwrap(), 4);
    r.end().unwrap();
}

#[test]
fn an_int_takes_as_many_bytes_as_its_magnitude_needs_whatever_its_sign() {
    let len = |n: i64| written(|w| w.int(n)).len() - FORMAT.len();
    assert_eq!((len(0), len(-1), len(63), len(-64)), (2, 2, 2, 2));
    assert_eq!((len(64), len(-65)), (3, 3));
    assert_eq!((len(i64::MAX), len(i64::MIN)), (11, 11));
}

#[test]
fn a_field_name_repeated_in_every_row_is_written_once_and_later_rows_refer_to_it_by_index() {
    let rows = |n: u64| {
        written(|w| {
            w.count(tag::LIST, n);
            for i in 0..n {
                w.count(tag::RECORD, 1);
                w.name(b"a_rather_long_field_name");
                w.int(i as i64);
            }
        })
    };
    let (small, large) = (rows(1), rows(101));
    assert!(
        large.len() - small.len() < 100 * 6,
        "each further row costs its tags, its count, the name's number and the int: {} then {}",
        small.len(),
        large.len()
    );
    let mut r = read(&large);
    r.expect(tag::LIST).unwrap();
    assert_eq!(r.count().unwrap(), 101);
    for i in 0..101 {
        r.expect(tag::RECORD).unwrap();
        assert_eq!(r.count().unwrap(), 1);
        assert_eq!(r.symbol().unwrap().as_str(), "a_rather_long_field_name");
        r.expect(tag::INT).unwrap();
        assert_eq!(r.int().unwrap(), i);
    }
    r.end().unwrap();
}

#[test]
fn the_reader_rejects_empty_input_a_wrong_header_truncated_data_trailing_bytes_and_oversized_counts()
 {
    assert!(Reader::new(&b""[..]).is_err());
    assert!(Reader::new(&b"PLV2"[..]).is_err());
    assert!(read(FORMAT).byte().unwrap_err().contains("ends early"));

    let mut whole = written(|w| w.int(1));
    whole.push(0);
    let mut r = read(&whole);
    r.expect(tag::INT).unwrap();
    r.int().unwrap();
    assert!(r.end().unwrap_err().contains("follow the value"));

    let cut = written(|w| w.str(b"a string that is cut short"));
    let mut r = read(&cut[..cut.len() - 3]);
    r.expect(tag::STR).unwrap();
    assert!(r.str().unwrap_err().contains("fewer bytes left"));

    let huge = with(&[tag::LIST, 0xff, 0xff, 0xff, 0xff, 0x0f]);
    let mut r = read(&huge);
    r.expect(tag::LIST).unwrap();
    assert!(r.count().unwrap_err().contains("fewer bytes left"));

    let unknown = with(&[200]);
    assert!(
        read(&unknown)
            .expect(tag::INT)
            .unwrap_err()
            .contains("tagged 200")
    );
    assert!(
        read(&with(&[0xff; 10]))
            .varint()
            .unwrap_err()
            .contains("64 bits")
    );
    assert!(read(&with(&[200, 0])).fixed().is_err(), "no width 200");
    assert!(
        read(&with(&[0x80, 0xb0, 0x03])).char().is_err(),
        "a surrogate"
    );
    assert!(read(&with(&[1, 0xff])).str().unwrap_err().contains("UTF-8"));
    assert!(
        read(&with(&[0, 1, 0xff])).symbol().is_err(),
        "a name not UTF-8"
    );
}

#[test]
fn reading_a_name_reference_to_an_index_that_was_never_defined_is_rejected() {
    let bytes = written(|w| {
        w.name(b"first");
        w.varint(2);
    });
    let mut r = read(&bytes);
    assert_eq!(r.symbol().unwrap().as_str(), "first");
    assert!(r.symbol().unwrap_err().contains("before it was written"));
}
