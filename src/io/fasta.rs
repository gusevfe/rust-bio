// Copyright 2014 Johannes Köster, Christopher Schröder.
// Licensed under the MIT license (http://opensource.org/licenses/MIT)
// This file may not be copied, modified, or distributed
// except according to those terms.


//! Fasta reading and writing.
//!
//! # Example
//!
//! ```
//! use std::io;
//! use bio::io::fasta;
//! let reader = fasta::Reader::new(io::stdin());
//! ```


use std::io;
use std::cmp;
use std::io::prelude::*;
use std::ascii::AsciiExt;
use std::collections;
use std::fs;
use std::path::Path;
use std::convert::AsRef;

use csv;


pub struct Reader<R: io::Read> {
    reader: io::BufReader<R>,
    line: String
}


impl Reader<fs::File> {
    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        fs::File::open(path).map(|f| Reader::new(f))
    }
}


impl<R: io::Read> Reader<R> {
    /// Create a new FastQ reader.
    pub fn new(reader: R) -> Self {
        Reader { reader: io::BufReader::new(reader), line: String::new() }
    }

    pub fn read(&mut self, record: &mut Record) -> io::Result<()> {
        record.clear();
        if self.line.is_empty() {
            try!(self.reader.read_line(&mut self.line));
            if self.line.is_empty() {
                return Ok(());
            }
        }

        if !self.line.starts_with(">") {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Expected > at record start."
            ));
        }
        record.header.push_str(&self.line);
        loop {
            self.line.clear();
            try!(self.reader.read_line(&mut self.line));
            if self.line.is_empty() || self.line.starts_with(">") {
                break;
            }
            record.seq.push_str(&self.line.trim_right());
        }

        Ok(())
    }

    /// Return an iterator over the records of this FastQ file.
    pub fn records(self) -> Records<R> {
        Records { reader: self }
    }
}


pub struct Index {
    inner: collections::HashMap<Vec<u8>, IndexRecord>,
    seqs: Vec<Vec<u8>>,
}


impl Index {
    pub fn new<R: io::Read>(fai: R) -> csv::Result<Self> {
        let mut inner = collections::HashMap::new();
        let mut seqs = vec![];
        let mut fai_reader = csv::Reader::from_reader(fai).delimiter(b'\t').has_headers(false);
        for row in fai_reader.decode() {
            let (name, record): (String, IndexRecord) = try!(row);
            seqs.push(name.clone().into_bytes());
            inner.insert(name.into_bytes(), record);
        }
        Ok(Index { inner: inner, seqs: seqs })
    }

    pub fn from_file<P: AsRef<Path>>(path: &P) -> csv::Result<Self> {
        match fs::File::open(path) {
            Ok(fai) => Self::new(fai),
            Err(e)  => Err(csv::Error::Io(e))
        }
    }

    pub fn with_fasta_file<P: AsRef<Path>>(fasta_path: &P) -> csv::Result<Self> {
        let mut ext = fasta_path.as_ref().extension().unwrap().to_str().unwrap().to_string();
        ext.push_str(".fai");
        let fai_path = fasta_path.as_ref().with_extension(ext);

        Self::from_file(&fai_path)
    }

    pub fn sequences(&self) -> Vec<Sequence> {
        self.seqs.iter().map(|name| Sequence { name: name.clone(), len: self.inner.get(name).unwrap().len }).collect()
    }
}


pub struct IndexedReader<R: io::Read + io::Seek> {
    reader: io::BufReader<R>,
    pub index: Index,
}


impl IndexedReader<fs::File> {
    pub fn from_file<P: AsRef<Path>>(path: &P) -> csv::Result<Self> {
        let index = try!(Index::with_fasta_file(path));

        match fs::File::open(path) {
            Ok(fasta) => Ok(IndexedReader::with_index(fasta, index)),
            Err(e)    => Err(csv::Error::Io(e))
        }
    }
}


impl<R: io::Read + io::Seek> IndexedReader<R> {
    pub fn new<I: io::Read>(fasta: R, fai: I) -> csv::Result<Self> {
        let index = try!(Index::new(fai));
        Ok(IndexedReader { reader: io::BufReader::new(fasta), index: index })
    }

    pub fn with_index(fasta: R, index: Index) -> Self {
        IndexedReader { reader: io::BufReader::new(fasta), index: index }
    }

    pub fn read_all(&mut self, seqname: &[u8], seq: &mut Vec<u8>) -> io::Result<()> {
        match self.index.inner.get(seqname) {
            Some(&idx) => self.read(seqname, 0, idx.len, seq),
            None      => Err(
                io::Error::new(
                    io::ErrorKind::Other,
                    "Unknown sequence name."
                )
            )
        }
    }

    pub fn read(&mut self, seqname: &[u8], start: u64, stop: u64, seq: &mut Vec<u8>) -> io::Result<()> {
        match self.index.inner.get(seqname) {
            Some(idx) => {
                seq.clear();
                // derived from
                // http://www.allenyu.info/item/24-quickly-fetch-sequence-from-samtools-faidx-indexed-fasta-sequences.html
                let line = start / idx.line_bases * idx.line_bytes;
                let line_offset = start % idx.line_bases;
                let offset = idx.offset + line + line_offset;
                let end = cmp::min(stop, idx.len);

                try!(self.reader.seek(io::SeekFrom::Start(offset)));
                let mut buf = vec![0u8; 1];
                while seq.len() < (end - start) as usize {
                    // read full lines
                    match self.reader.read(&mut buf) {
                      Ok(n) => {
                        if buf[0] != 0xA && buf[0] != 13 {
                          seq.push_all(&buf);
                        }
                      },
                      Err(e) => {
                        return Err(e);
                      }
                    }
                }
                // read last line
                Ok(())
            },
            None      => {
              Err(
                io::Error::new(
                    io::ErrorKind::Other,
                    "Unknown sequence name."
                )
            )
            }
        }
    }
}


#[derive(RustcDecodable, Debug, Copy, Clone)]
struct IndexRecord {
    len: u64,
    offset: u64,
    line_bases: u64,
    line_bytes: u64,
}


pub struct Sequence {
    pub name: Vec<u8>,
    pub len: u64,
}


/// A Fasta writer.
pub struct Writer<W: io::Write> {
    writer: io::BufWriter<W>,
}


impl Writer<fs::File> {
    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        fs::File::create(path).map(|f| Writer::new(f))
    }
}


impl<W: io::Write> Writer<W> {
    /// Create a new Fasta writer.
    pub fn new(writer: W) -> Self {
        Writer { writer: io::BufWriter::new(writer) }
    }

    /// Directly write a Fasta record.
    pub fn write_record(&mut self, record: Record) -> io::Result<()> {
        self.write(record.id().unwrap_or(""), record.desc(), record.seq())
    }

    /// Write a Fasta record with given values.
    ///
    /// # Arguments
    ///
    /// * `id` - the record id
    /// * `desc` - the optional descriptions
    /// * `seq` - the sequence
    pub fn write(&mut self, id: &str, desc: Option<&str>, seq: &[u8]) -> io::Result<()> {
        try!(self.writer.write(b">"));
        try!(self.writer.write(id.as_bytes()));
        if desc.is_some() {
            try!(self.writer.write(b" "));
            try!(self.writer.write(desc.unwrap().as_bytes()));
        }
        try!(self.writer.write(b"\n"));
        try!(self.writer.write(seq));
        try!(self.writer.write(b"\n"));

        Ok(())
    }

    /// Flush the writer, ensuring that everything is written.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}


pub struct Record {
    header: String,
    seq: String,
}


impl Record {
    pub fn new() -> Self {
        Record { header: String::new(), seq: String::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.header.is_empty() && self.seq.is_empty()
    }

    /// Check validity of Fasta record.
    pub fn check(&self) -> Result<(), &str> {
        if self.id().is_none() {
            return Err("Expecting id for FastQ record.");
        }
        if !self.seq.is_ascii() {
            return Err("Non-ascii character found in sequence.");
        }

        Ok(())
    }

    /// Return the id of the record.
    pub fn id(&self) -> Option<&str> {
        self.header[1..].trim_right().splitn(2, " ").next()
    }

    /// Return descriptions if present.
    pub fn desc(&self) -> Option<&str> {
        self.header[1..].trim_right().splitn(2, " ").skip(1).next()
    }

    /// Return the sequence of the record.
    pub fn seq(&self) -> &[u8] {
        self.seq.as_bytes()
    }

    fn clear(&mut self) {
        self.header.clear();
        self.seq.clear();
    }
}


/// An iterator over the records of a Fasta file.
pub struct Records<R: io::Read> {
    reader: Reader<R>,
}


impl<R: io::Read> Iterator for Records<R> {
    type Item = io::Result<Record>;

    fn next(&mut self) -> Option<io::Result<Record>> {
        let mut record = Record::new();
        match self.reader.read(&mut record) {
            Ok(()) if record.is_empty() => None,
            Ok(())   => Some(Ok(record)),
            Err(err) => Some(Err(err))
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    const FASTA_FILE: &'static [u8] = b">id desc
ACCGTAGGCTGA
>id2
ATTGTTGTTTTA
";
    const MULTILINE_FASTA_FILE: &'static [u8] = b">name1
AATCAGTTTGGTGGTCGTGG
GGTGCCCCCTCCGACTTAGA
GCTAACCGAA
>name2
GGACGAGCAGTATCACACAC
GGGTAGATGTTTACGGCTAG
CAATGGCGTT
";
    const FAI_FILE: &'static [u8] = b"name1\t50\t7\t20\t21\nname2\t50\t67\t20\t21\n";

    #[test]
    fn test_reader() {
        let reader = Reader::new(FASTA_FILE);
        let ids = [Some("id"), Some("id2")];
        let descs = [Some("desc"), None];
        let seqs : [&[u8]; 2] = [b"ACCGTAGGCTGA", b"ATTGTTGTTTTA"];

        for (i, r) in reader.records().enumerate() {
          let record = r.ok().expect("Error reading record");
          assert_eq!(record.check(), Ok(()));
          assert_eq!(record.id(), ids[i]);
          assert_eq!(record.desc(), descs[i]);
          assert_eq!(record.seq(), seqs[i]);
        }


        // let record = records.ok().nth(1).unwrap();
    }

    #[test]
    fn test_indexed_reader() {
        let mut reader = IndexedReader::new(io::Cursor::new(MULTILINE_FASTA_FILE), FAI_FILE).ok().expect("Error reading index");
        let mut seq = Vec::new();
        reader.read(b"name1", 1, 5, &mut seq).ok().expect("Error reading sequence.");
        assert_eq!(seq, b"ATCA");

        // Read through newline
        reader.read(b"name2", 10, 30, &mut seq).ok().expect("Error reading sequence.");
        assert_eq!(seq, b"TATCACACACGGGTAGATGT");

        // Read past end of sequence
        reader.read(b"name1", 40, 60, &mut seq).ok().expect("Error reading sequence.");
        assert_eq!(seq, b"GCTAACCGAA");
    }

    #[test]
    fn test_writer() {
        let mut writer = Writer::new(Vec::new());
        writer.write("id", Some("desc"), b"ACCGTAGGCTGA").ok().expect("Expected successful write");
        writer.write("id2", None, b"ATTGTTGTTTTA").ok().expect("Expected successful write");
        writer.flush().ok().expect("Expected successful write");
        assert_eq!(writer.writer.get_ref(), &FASTA_FILE);
    }
}
