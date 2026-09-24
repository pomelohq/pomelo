//! Terminal query handling. A holder has no emulator of its own, yet programs probe the terminal (device
//! attributes, cursor position, version) and some block until answered, so the holder answers the common
//! ones itself. Replayed scrollback must not re-trigger queries, so snapshots drop them.

/// Removes the queries the holder answers from `bytes` and returns (output to publish, reply to write back
/// to the program, an incomplete trailing escape to prepend to the next read).
pub fn answer_queries(bytes: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    const ANSWERS: &[(&[u8], &[u8], u8)] = &[
        (b"\x1b[>0q", b"\x1bP>|ptyhost 1.0\x1b\\", b'q'),
        (b"\x1b[>q", b"\x1bP>|ptyhost 1.0\x1b\\", b'q'),
        (b"\x1b[?u", b"\x1b[?0u", 0),
        (b"\x1b[>c", b"\x1b[>0;276;0c", b'c'),
        (b"\x1b[>0c", b"\x1b[>0;276;0c", b'c'),
        (b"\x1b[c", b"\x1b[?64;1;2;6;9;15;18;21;22c", b'c'),
        (b"\x1b[0c", b"\x1b[?64;1;2;6;9;15;18;21;22c", b'c'),
        (b"\x1b[5n", b"\x1b[0n", 0),
        (b"\x1b[6n", b"\x1b[1;1R", 0),
    ];
    let mut output = Vec::with_capacity(bytes.len());
    let mut reply = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        let rest = &bytes[index..];
        let answered = ANSWERS.iter().find(|(query, _, _)| rest.starts_with(query));
        match answered {
            Some((query, answer, final_byte)) => {
                reply.extend_from_slice(answer);
                index += if *final_byte == 0 {
                    query.len()
                } else {
                    query_len(rest, *final_byte)
                };
            }
            None if is_partial_query(rest) => return (output, reply, rest.to_vec()),
            None => {
                output.push(bytes[index]);
                index += 1;
            }
        }
    }
    (output, reply, Vec::new())
}

fn query_len(bytes: &[u8], final_byte: u8) -> usize {
    (2..bytes.len().min(12))
        .find(|&index| bytes[index] == final_byte)
        .map_or(3, |index| index + 1)
}

fn is_partial_query(bytes: &[u8]) -> bool {
    let has_final = bytes
        .iter()
        .any(|&byte| (0x40..=0x7e).contains(&byte) && byte != b'[');
    bytes.len() < 6 && bytes.first() == Some(&0x1b) && !has_final
}

/// Scrubs device queries (DA, DSR, DECRQM, XTVERSION, OSC color/clipboard queries) from replayed
/// scrollback, so a reattaching emulator doesn't answer them again into the shell prompt.
pub fn strip_device_queries(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b && index + 1 < bytes.len() {
            match bytes[index + 1] {
                b'[' => {
                    let mut end = index + 2;
                    while end < bytes.len() && (0x20..=0x3f).contains(&bytes[end]) {
                        end += 1;
                    }
                    if end < bytes.len() && csi_is_query(&bytes[index..=end]) {
                        index = end + 1;
                        continue;
                    }
                }
                b']' => {
                    let mut end = index + 2;
                    while end < bytes.len() {
                        if bytes[end] == 0x07 {
                            end += 1;
                            break;
                        }
                        if bytes[end] == 0x1b && bytes.get(end + 1) == Some(&b'\\') {
                            end += 2;
                            break;
                        }
                        end += 1;
                    }
                    if bytes[index..end].windows(2).any(|pair| pair == b";?") {
                        index = end;
                        continue;
                    }
                }
                _ => {}
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    output
}

fn csi_is_query(sequence: &[u8]) -> bool {
    match sequence.last() {
        Some(b'c' | b'n') => true,
        Some(b'p') => sequence.contains(&b'$'),
        // DECSCUSR (cursor shape) also ends in `q` but uses a space intermediate, not `>`.
        Some(b'q') => sequence.contains(&b'>'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_and_removes_known_queries() {
        let (output, reply, carry) = answer_queries(b"a\x1b[6nb\x1b[cc");
        assert_eq!(output, b"abc");
        assert_eq!(reply, b"\x1b[1;1R\x1b[?64;1;2;6;9;15;18;21;22c");
        assert!(carry.is_empty());
    }

    #[test]
    fn keeps_other_escapes_and_carries_a_split_query() {
        let (output, reply, carry) = answer_queries(b"\x1b[31mred\x1b[");
        assert_eq!(output, b"\x1b[31mred");
        assert!(reply.is_empty());
        assert_eq!(carry, b"\x1b[");
        let mut next = carry;
        next.extend_from_slice(b"5n");
        let (output, reply, _) = answer_queries(&next);
        assert!(output.is_empty());
        assert_eq!(reply, b"\x1b[0n");
    }

    #[test]
    fn strips_queries_from_snapshots_only() {
        let snapshot = b"x\x1b[6n\x1b[?2026$p\x1b[>0q\x1b[2 q\x1b]11;?\x07y\x1b[1mz";
        assert_eq!(strip_device_queries(snapshot), b"x\x1b[2 qy\x1b[1mz");
    }
}
