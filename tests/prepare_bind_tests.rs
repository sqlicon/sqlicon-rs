use sqlicon_rs::statement::ParameterValue;
use sqlicon_rs::types::date::Date;
use bytes::BytesMut;

#[test]
fn test_parameter_value_encodings() {
    // 1. Null
    let p_null = ParameterValue::Null;
    let mut b_null = BytesMut::new();
    p_null.encode_into(&mut b_null);
    assert_eq!(&b_null[..], &[0, 0, 0xff, 0xff, 0, 0]);

    // 2. Int
    let p_int = ParameterValue::Int(42);
    let mut b_int = BytesMut::new();
    p_int.encode_into(&mut b_int);
    assert_eq!(&b_int[..6], &[0, 2, 0, 0, 0, 0]);
    assert_eq!(&b_int[6..10], &42i32.to_be_bytes());

    // 3. BigInt
    let p_big = ParameterValue::BigInt(9999999999);
    let mut b_big = BytesMut::new();
    p_big.encode_into(&mut b_big);
    assert_eq!(&b_big[..6], &[0, 52, 0, 0, 0, 0]);
    assert_eq!(&b_big[6..14], &9999999999i64.to_be_bytes());

    // 4. Float
    let p_flt = ParameterValue::Float(3.14159);
    let mut b_flt = BytesMut::new();
    p_flt.encode_into(&mut b_flt);
    assert_eq!(&b_flt[..6], &[0, 3, 0, 0, 0, 0]);
    assert_eq!(&b_flt[6..14], &3.14159f64.to_be_bytes());

    // 5. String
    let p_str = ParameterValue::String("hello".into());
    let mut b_str = BytesMut::new();
    p_str.encode_into(&mut b_str);
    assert_eq!(&b_str[..4], &[0, 13, 0, 0]); // type 13, null 0
    assert_eq!(&b_str[4..6], &5u16.to_be_bytes()); // length
    assert_eq!(&b_str[6..8], &5u16.to_be_bytes()); // length
    assert_eq!(&b_str[8..13], b"hello");
    assert_eq!(b_str[13], 0); // padding

    // 6. Date
    let d = Date::new(1970, 1, 1).unwrap();
    let p_date = ParameterValue::Date(d);
    let mut b_date = BytesMut::new();
    p_date.encode_into(&mut b_date);
    assert_eq!(&b_date[..6], &[0, 7, 0, 0, 0, 0]);
    assert_eq!(&b_date[6..10], &[0, 0, 0x63, 0xe0]);
}
