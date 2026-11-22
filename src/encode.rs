//! N-Quads / N-Triples encoding, off the producer threads.
//!
//! Producers send structured batches; a pool of encoder threads turns them
//! into lines and forwards bytes to the writer. Line-based RDF is a set, so
//! batches may be encoded and written in any order. The encoder writes bytes
//! directly instead of going through `fmt`, and produces exactly the bytes of
//! oxrdf's `Display` (and so of oxttl's N-Quads serializer).

use {
    crate::{
        emit::{Object, QuadBatch, Triple},
        vocab::xsd,
    },
    oxrdf::{LiteralRef, NamedNode, NamedNodeRef},
    std::thread::JoinHandle,
};

/// Line-based encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    NQuads,
    /// The union of all graphs.
    NTriples,
}

impl Encoding {
    /// Encode a structured batch; encoded batches pass through.
    pub fn encode(self, batch: QuadBatch) -> QuadBatch {
        let QuadBatch::Triples { graph, triples } = batch else {
            return batch;
        };
        let graph = (self == Self::NQuads).then_some(graph.as_ref());
        let mut bytes = Vec::with_capacity(triples.len() * 160);
        for t in &triples {
            line(&mut bytes, t, graph);
        }
        QuadBatch::Encoded { bytes, quads: triples.len() as u64 }
    }

    /// Start `threads` encoders between `input` and `output`. They stop when
    /// every producer has dropped its sender.
    pub fn spawn_pool(
        self,
        threads: usize,
        input: flume::Receiver<QuadBatch>,
        output: flume::Sender<QuadBatch>,
    ) -> Vec<JoinHandle<()>> {
        (0..threads.max(1))
            .map(|i| {
                let (input, output) = (input.clone(), output.clone());
                std::thread::Builder::new()
                    .name(format!("encode-{i}"))
                    .spawn(move || {
                        for batch in input.iter() {
                            if output.send(self.encode(batch)).is_err() {
                                break; // writer gone; it reports why
                            }
                        }
                    })
                    .expect("spawn encoder thread")
            })
            .collect()
    }
}

fn line(buf: &mut Vec<u8>, t: &Triple, graph: Option<&NamedNode>) {
    iri(buf, t.subject.as_ref().as_ref());
    buf.push(b' ');
    iri(buf, t.predicate);
    buf.push(b' ');
    match &t.object {
        Object::Static(n) => iri(buf, *n),
        Object::Node(n) => iri(buf, n.as_ref().as_ref()),
        Object::Literal(l) => literal(buf, l.as_ref()),
    }
    if let Some(g) = graph {
        buf.push(b' ');
        iri(buf, g.as_ref());
    }
    buf.extend_from_slice(b" .\n");
}

fn iri(buf: &mut Vec<u8>, n: NamedNodeRef<'_>) {
    buf.push(b'<');
    buf.extend_from_slice(n.as_str().as_bytes());
    buf.push(b'>');
}

fn literal(buf: &mut Vec<u8>, l: LiteralRef<'_>) {
    quoted(buf, l.value());
    if let Some(lang) = l.language() {
        buf.push(b'@');
        buf.extend_from_slice(lang.as_bytes());
    } else if l.datatype() != xsd::STRING {
        buf.extend_from_slice(b"^^");
        iri(buf, l.datatype());
    }
}

/// Whether `s` contains anything oxrdf escapes: control characters, `"`,
/// `\`, DEL, or U+FFFE/U+FFFF (UTF-8 `EF BF BE`/`EF BF BF`).
fn needs_escape(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().any(|(i, &c)| {
        c < 0x20 ||
            c == b'"' ||
            c == b'\\' ||
            c == 0x7F ||
            (c == 0xEF &&
                b.get(i + 1) == Some(&0xBF) &&
                matches!(b.get(i + 2), Some(0xBE | 0xBF)))
    })
}

/// Same escaping as `oxrdf::literal::print_quoted_str`.
fn quoted(buf: &mut Vec<u8>, s: &str) {
    buf.push(b'"');
    if needs_escape(s) {
        for c in s.chars() {
            match c {
                '\u{8}' => buf.extend_from_slice(b"\\b"),
                '\t' => buf.extend_from_slice(b"\\t"),
                '\n' => buf.extend_from_slice(b"\\n"),
                '\u{C}' => buf.extend_from_slice(b"\\f"),
                '\r' => buf.extend_from_slice(b"\\r"),
                '"' => buf.extend_from_slice(b"\\\""),
                '\\' => buf.extend_from_slice(b"\\\\"),
                '\0'..='\u{1F}' | '\u{7F}' | '\u{FFFE}' | '\u{FFFF}' => {
                    buf.extend_from_slice(
                        format!("\\u{:04X}", u32::from(c)).as_bytes(),
                    );
                },
                _ => {
                    let mut tmp = [0u8; 4];
                    buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
                },
            }
        }
    } else {
        buf.extend_from_slice(s.as_bytes());
    }
    buf.push(b'"');
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{emit::Node, vocab::raw},
        oxrdf::{Literal, QuadRef},
        std::sync::Arc,
    };

    fn node(s: &str) -> Node { Arc::new(NamedNode::new_unchecked(s)) }

    /// Every literal shape and every escaped character encodes exactly as
    /// oxrdf's `Display`.
    #[test]
    fn matches_oxrdf_display() {
        let tricky = "plain é ü 中文 \u{8}\t\n\u{C}\r\"\\ \
                      \0\u{1}\u{1F}\u{7F}\u{FFFE}\u{FFFF}\u{FFFD} end";
        let literals = [
            Literal::new_simple_literal("00123"),
            Literal::new_simple_literal(tricky),
            Literal::new_simple_literal(""),
            Literal::new_language_tagged_literal_unchecked(tricky, "fr"),
            Literal::new_typed_literal("12.50", xsd::DECIMAL),
            Literal::from(1.5_f64),
            Literal::from(true),
        ];
        let graph = NamedNode::new_unchecked(
            "urn:uuid:00000000-0000-8000-8000-000000000000",
        );
        let s = node("urn:uuid:11111111-1111-8111-8111-111111111111");
        let objects = literals
            .into_iter()
            .map(Object::Literal)
            .chain([Object::Static(raw::VALUE), Object::Node(node("urn:x:y"))]);
        for object in objects {
            let t = Triple {
                subject: s.clone(),
                predicate: raw::LITERAL_VALUE,
                object,
            };
            for g in [Some(&graph), None] {
                let mut ours = Vec::new();
                line(&mut ours, &t, g);
                let theirs = format!(
                    "{} .\n",
                    QuadRef::new(
                        t.subject.as_ref().as_ref(),
                        t.predicate,
                        match &t.object {
                            Object::Static(n) => oxrdf::TermRef::from(*n),
                            Object::Node(n) =>
                                oxrdf::TermRef::from(n.as_ref().as_ref()),
                            Object::Literal(l) =>
                                oxrdf::TermRef::from(l.as_ref()),
                        },
                        g.map_or(oxrdf::GraphNameRef::DefaultGraph, |g| {
                            g.as_ref().into()
                        }),
                    )
                );
                assert_eq!(String::from_utf8(ours).unwrap(), theirs);
            }
        }
    }
}
