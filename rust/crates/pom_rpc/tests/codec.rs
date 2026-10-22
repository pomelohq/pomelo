use std::io::{self, Cursor, Read};

use pom_rpc::{
    decode, read_frame, write_frame, End, Error, Frame, Hello, Kind, Push, ReadError, Request,
    Response, MAX_DATA, MAX_FRAME,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

fn encode_all(frames: &[Frame]) -> Vec<u8> {
    let mut buffer = Vec::new();
    for frame in frames {
        write_frame(&mut buffer, frame).unwrap();
    }
    buffer
}

/// Hands out at most `chunk` bytes per read, like a slow socket.
struct Trickle {
    bytes: Cursor<Vec<u8>>,
    chunk: usize,
}

impl Read for Trickle {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let limit = self.chunk.min(buffer.len());
        self.bytes.read(&mut buffer[..limit])
    }
}

#[test]
fn every_kind_round_trips() {
    for (index, kind) in Kind::ALL.into_iter().enumerate() {
        assert_eq!(Kind::from_code(kind.code()), Some(kind));
        let frame = Frame::new(
            kind,
            0xDEAD_0000 + index as u32,
            vec![index as u8; index * 3],
        );
        let bytes = encode_all(std::slice::from_ref(&frame));
        assert_eq!(&bytes[0..4], &((5 + index * 3) as u32).to_be_bytes());
        assert_eq!(bytes[4], kind.code());
        let decoded = read_frame(&mut Cursor::new(bytes)).unwrap().unwrap();
        assert_eq!(decoded, frame);
    }
}

#[test]
fn kind_codes_match_the_wire_table() {
    let codes: Vec<u8> = Kind::ALL.into_iter().map(Kind::code).collect();
    assert_eq!(
        codes,
        [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x10, 0x11, 0x12, 0x13, 0x20, 0x21]
    );
}

#[test]
fn frames_split_across_reads_reassemble() {
    let frames = vec![
        Frame::json(Kind::Request, 1, &json!({"method": "a", "params": [1, 2]})).unwrap(),
        Frame::bytes(Kind::Data, 3, 42, b"hello world").unwrap(),
        Frame::new(Kind::Ping, 9, Vec::new()),
        Frame::offset_only(Kind::Ack, 3, 53),
    ];
    for chunk in [1, 2, 3, 7] {
        let mut reader = Trickle {
            bytes: Cursor::new(encode_all(&frames)),
            chunk,
        };
        for expected in &frames {
            assert_eq!(&read_frame(&mut reader).unwrap().unwrap(), expected);
        }
        assert!(read_frame(&mut reader).unwrap().is_none());
    }
}

#[test]
fn end_of_stream_inside_a_frame_is_an_error() {
    let bytes = encode_all(&[Frame::new(Kind::Push, 1, b"{}".to_vec())]);
    for cut in 1..bytes.len() {
        let result = read_frame(&mut Cursor::new(bytes[..cut].to_vec()));
        assert!(matches!(result, Err(ReadError::Io(_))), "cut at {cut}");
    }
}

#[test]
fn oversized_frame_is_rejected_before_reading_it() {
    let mut bytes = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(&[Kind::Data.code(), 0, 0, 0, 1]);
    match read_frame(&mut Cursor::new(bytes)) {
        Err(ReadError::Protocol(message)) => assert!(message.contains("at most")),
        other => panic!("expected a protocol error, got {other:?}"),
    }
    let frame = Frame::new(Kind::Push, 1, vec![0; MAX_FRAME]);
    assert!(matches!(frame.encode(), Err(Error::Limit(_))));
    assert!(Frame::new(Kind::Push, 1, vec![0; MAX_FRAME - 5])
        .encode()
        .is_ok());
}

#[test]
fn unknown_kind_is_rejected() {
    let mut bytes = 5u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(&[0x7f, 0, 0, 0, 1]);
    match read_frame(&mut Cursor::new(bytes)) {
        Err(ReadError::Protocol(message)) => assert!(message.contains("0x7f")),
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

#[test]
fn length_shorter_than_header_is_rejected() {
    let mut bytes = 4u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(&[Kind::Ping.code(), 0, 0, 0]);
    assert!(matches!(
        read_frame(&mut Cursor::new(bytes)),
        Err(ReadError::Protocol(_))
    ));
}

#[test]
fn data_payload_carries_its_offset_and_respects_the_limit() {
    let frame = Frame::bytes(Kind::Input, 5, u64::MAX - 3, b"abc").unwrap();
    assert_eq!(
        frame.offset_and_bytes().unwrap(),
        (u64::MAX - 3, &b"abc"[..])
    );
    assert!(Frame::bytes(Kind::Data, 5, 0, &vec![0; MAX_DATA]).is_ok());
    assert!(matches!(
        Frame::bytes(Kind::Data, 5, 0, &vec![0; MAX_DATA + 1]),
        Err(Error::Limit(_))
    ));
    let mut payload = 0u64.to_be_bytes().to_vec();
    payload.extend(vec![0; MAX_DATA + 1]);
    assert!(Frame::new(Kind::Data, 5, payload)
        .offset_and_bytes()
        .is_err());
    assert!(Frame::new(Kind::Ack, 5, vec![1, 2, 3]).offset().is_err());
    assert_eq!(Frame::offset_only(Kind::Ack, 5, 77).offset().unwrap(), 77);
}

#[test]
fn malformed_json_is_a_protocol_error() {
    assert!(matches!(
        decode::<Request>(b"{\"method\": "),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        decode::<Hello>(b"[1, 2]"),
        Err(Error::Protocol(_))
    ));
}

#[derive(Serialize)]
struct NewerHello {
    protocol: u32,
    pom_version: String,
    server_id: String,
    features: Vec<String>,
    compression: String,
    limits: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct NewerResponse {
    ok: serde_json::Value,
    elapsed_ms: u64,
}

#[test]
fn newer_structs_with_extra_fields_decode_on_the_old_ones() {
    let newer = NewerHello {
        protocol: 1,
        pom_version: "9.9.9".to_string(),
        server_id: "server".to_string(),
        features: vec!["pty".to_string()],
        compression: "zstd".to_string(),
        limits: json!({"window": 1}),
    };
    let hello: Hello = decode(&serde_json::to_vec(&newer).unwrap()).unwrap();
    assert_eq!(hello.pom_version, "9.9.9");
    assert_eq!(hello.server_id.as_deref(), Some("server"));
    assert_eq!(hello.client_id, None);
    assert_eq!(hello.features, ["pty"]);

    let response: Response = decode(
        &serde_json::to_vec(&NewerResponse {
            ok: json!([1]),
            elapsed_ms: 3,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(response.into_result().unwrap(), json!([1]));

    let request: Request = decode(br#"{"method":"ws.list","params":{"a":1},"trace":"x"}"#).unwrap();
    assert_eq!(request.method, "ws.list");
    let push: Push = decode(br#"{"topic":"t","value":2,"sequence":9}"#).unwrap();
    assert_eq!(push.value, json!(2));
    let end: End = decode(br#"{"reason":"done","code":4}"#).unwrap();
    assert_eq!(end.reason, "done");
    let error: Response =
        decode(br#"{"err":{"code":"cancelled","message":"m","retry":true}}"#).unwrap();
    assert_eq!(error.into_result().unwrap_err().code, "cancelled");
    let empty: Hello = decode(b"{}").unwrap();
    assert_eq!(empty, Hello::default());
}
