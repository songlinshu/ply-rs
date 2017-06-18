use std::io::{ Read, BufReader, BufRead, Result, Error, ErrorKind };
use std::fmt::Debug;
use std::result;

use byteorder::{ BigEndian, LittleEndian };

use grammar;
use ply::*;
use util::LocationTracker;

mod ascii;
mod binary;

#[derive(Debug, PartialEq, Clone)]
pub enum Line {
    MagicNumber,
    Format((Encoding, Version)),
    Comment(Comment),
    ObjInfo(ObjInfo),
    Element(ElementDef),
    Property(PropertyDef),
    EndHeader
}

macro_rules! is_line {
    ($e:expr, $t:ty) => (
        match $e {
            Err(e) => return Err(Error::new(ErrorKind::InvalidInput, e)),
            Ok(l @ Line::MagicNumber) => (l),
            Ok(ob) => return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("Invalid line encountered. Expected type: '$t', found: '{:?}'", ob)
            )),
        }
    );
}


fn parse_ascii_rethrow<T, E: Debug>(location: &LocationTracker, line_str: &str, e: E, message: &str) -> Result<T> {
    Err(Error::new(
        ErrorKind::InvalidInput,
        format!("Line {}: {}\n\tString: '{}'\n\tError: {:?}", location.line_index, message, line_str, e)
    ))
}
fn parse_ascii_error<T>(location: &LocationTracker, line_str: &str, message: &str) -> Result<T> {
    Err(Error::new(
        ErrorKind::InvalidInput,
        format!("Line {}: {}\n\tString: '{}'", location.line_index, message, line_str)
    ))
}

use std::marker::PhantomData;
pub struct Parser<E: PropertyAccess> {
      phantom: PhantomData<E>,
}

impl<E: PropertyAccess> Parser<E> {
    pub fn new() -> Self {
        Parser {
            phantom: PhantomData
        }
    }
    pub fn read_ply<T: Read>(&self, source: &mut T) -> Result<Ply<E>> {
        let mut source = BufReader::new(source);
        let mut location = LocationTracker::new();
        let header = try!(self.__read_header(&mut source, &mut location));
        let payload = try!(self.__read_payload(&mut source, &mut location, &header));
        let mut ply = Ply::new();
        ply.header = header;
        ply.payload = payload;
        Ok(ply)
    }
    pub fn read_header<T: BufRead>(&self, reader: &mut T) -> Result<Header> {
        let mut line = LocationTracker::new();
        self.__read_header(reader, &mut line)
    }
    pub fn read_header_line(&self, line: &str) -> Result<Line> {
        match self.__read_header_line(line) {
            Ok(l) => Ok(l),
            Err(e) => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("Couldn't parse line.\n\tString: {}\n\tError: {:?}", line, e)
            )),
        }
    }
    pub fn read_payload_for_element<T: BufRead>(&self, reader: &mut T, element_def: &ElementDef, header: &Header) -> Result<Vec<E>> {
        let mut location = LocationTracker::new();
        match header.encoding {
            Encoding::Ascii => self.__read_ascii_payload_for_element(reader, &mut location, element_def),
            Encoding::BinaryBigEndian => self.__read_binary_payload_for_element::<T, BigEndian>(reader, &mut location, element_def),
            Encoding::BinaryLittleEndian => self.__read_binary_payload_for_element::<T, LittleEndian>(reader, &mut location, element_def),
        }
    }
    pub fn read_big_endian_element<T: Read>(&self, reader: &mut T, element_def: &ElementDef) -> Result<E> {
        /// Reduce coupling with ByteOrder
        self.__read_binary_element::<T, BigEndian>(reader, element_def)

    }
    pub fn read_little_endian_element<T: Read>(&self, reader: &mut T, element_def: &ElementDef) -> Result<E> {
        /// Reduce coupling with ByteOrder
        self.__read_binary_element::<T, LittleEndian>(reader, element_def)
    }
    pub fn read_ascii_element(&self, line: &str, element_def: &ElementDef) -> Result<E> {
        self.__read_ascii_element(line, element_def)
    }

    // private
    fn __read_header_line(&self, line_str: &str) -> result::Result<Line, grammar::ParseError> {
        grammar::line(line_str)
    }
    fn __read_header<T: BufRead>(&self, reader: &mut T, location: &mut LocationTracker) -> Result<Header> {
        location.next_line();
        let mut line_str = String::new();
        try!(reader.read_line(&mut line_str));
        match self.__read_header_line(&line_str) {
            Ok(Line::MagicNumber) => (),
            Ok(l) => return parse_ascii_error(location, &line_str, &format!("Expected magic number 'ply', but saw '{:?}'.", l)),
            Err(e) => return parse_ascii_rethrow(location, &line_str, e, "Expected magic number 'ply'.")
        }
        is_line!(grammar::line(&line_str), Line::MagicNumber);

        let mut header_form_ver : Option<(Encoding, Version)> = None;
        let mut header_obj_infos = Vec::<ObjInfo>::new();
        let mut header_elements = KeyMap::<ElementDef>::new();
        let mut header_comments = Vec::<Comment>::new();
        location.next_line();
        'readlines: loop {
            line_str.clear();
            try!(reader.read_line(&mut line_str));
            let line = self.__read_header_line(&line_str);

            match line {
                Err(e) => return parse_ascii_rethrow(location, &line_str, e, "Couldn't parse line."),
                Ok(Line::MagicNumber) => return parse_ascii_error(location, &line_str, "Unexpected 'ply' found."),
                Ok(Line::Format(ref t)) => (
                    if header_form_ver.is_none() {
                        header_form_ver = Some(t.clone());
                    } else {
                        let f = header_form_ver.unwrap();
                        if f != *t {
                            return parse_ascii_error(
                                location,
                                &line_str,
                                &format!(
                                    "Found contradicting format definition:\n\
                                    \tEncoding: {:?}, Version: {:?}\n\
                                    previous definition:\n\
                                    \tEncoding: {:?}, Version: {:?}",
                                    t.0, t.1, f.0, f.1)
                            )
                        }
                    }
                ),
                Ok(Line::ObjInfo(ref o)) => (
                    header_obj_infos.push(o.clone())
                ),
                Ok(Line::Comment(ref c)) => (
                    header_comments.push(c.clone())
                ),
                Ok(Line::Element(ref e)) => {
                    header_elements.add(e.clone())
                },
                Ok(Line::Property(p)) => (
                    if header_elements.is_empty() {
                        return parse_ascii_error(
                            location,
                            &line_str,
                            &format!("Property '{:?}' found without preceding element.", p)
                        );
                    } else {
                        let (_, mut e) = header_elements.pop_back().unwrap();
                        e.properties.add(p);
                        header_elements.add(e);
                    }
                ),
                Ok(Line::EndHeader) => { location.next_line(); break 'readlines; },
            };
            location.next_line();
        }
        if header_form_ver.is_none() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "No format line found."
            ));
        }
        let (encoding, version) = header_form_ver.unwrap();
        Ok(Header{
            encoding: encoding,
            version: version,
            obj_infos: header_obj_infos,
            comments: header_comments,
            elements: header_elements
        })
    }
    /// internal dispatcher based on the encoding
    fn __read_payload<T: BufRead>(&self, reader: &mut T, location: &mut LocationTracker, header: &Header) -> Result<Payload<E>> {
        let mut payload = Payload::new();
        match header.encoding {
            Encoding::Ascii => for (k, ref e) in &header.elements {
                let elems = try!(self.__read_ascii_payload_for_element(reader, location, e));
                payload.insert(k.clone(), elems);
            },
            Encoding::BinaryBigEndian => for (k, ref e) in &header.elements {
                let elems = try!(self.__read_binary_payload_for_element::<T, BigEndian>(reader, location, e));
                payload.insert(k.clone(), elems);
            },
            Encoding::BinaryLittleEndian => for (k, ref e) in &header.elements {
                let elems = try!(self.__read_binary_payload_for_element::<T, LittleEndian>(reader, location, e));
                payload.insert(k.clone(), elems);
            }
        }
        Ok(payload)
    }
}


#[cfg(test)]
mod tests {
    use grammar as g;
    use super::*;
    macro_rules! assert_ok {
        ($e:expr) => (
            match $e {
                Ok(obj) => (obj),
                Err(e) => panic!("{}", e),
            }
        );
        ($e:expr , $o:expr) => (
            let obj = assert_ok!($e);
            assert_eq!(obj, $o);
        );
    }
    macro_rules! assert_err {
        ($e:expr) => (
            let result = $e;
            assert!(result.is_err());
        );
    }
    #[test]
    fn parser_header_ok(){
        let p = Parser::<DefaultElement>::new();
        let txt = "ply\nformat ascii 1.0\nend_header\n";
        let mut bytes = txt.as_bytes();
        assert_ok!(p.read_header(&mut bytes));

        let txt = "ply\n\
        format ascii 1.0\n\
        element vertex 8\n\
        property float x\n\
        property float y\n\
        element face 6\n\
        property list uchar int vertex_index\n\
        end_header\n";
        let mut bytes = txt.as_bytes();
        assert_ok!(p.read_header(&mut bytes));
    }
    #[test]
    fn parser_demo_ok(){
        let txt = "ply\nformat ascii 1.0\nend_header\n";
        let mut bytes = txt.as_bytes();
        let p = Parser::<DefaultElement>::new();
        assert_ok!(p.read_header(&mut bytes));

        let txt = "ply\n\
        format ascii 1.0\n\
        element vertex 1\n\
        property float x\n\
        end_header\n
        6.28318530718"; // no newline at end!
        let mut bytes = txt.as_bytes();
        assert_ok!(p.read_header(&mut bytes));
    }
    #[test]
    fn parser_single_elements_ok(){
        let txt = "ply\r\n\
        format ascii 1.0\r\n\
        comment Hi, I'm your friendly comment.\r\n\
        obj_info And I'm your object information.\r\n\
        element point 2\r\n\
        property int x\r\n\
        property int y\r\n\
        end_header\r\n\
        -7 5\r\n\
        2 4\r\n";
        let mut bytes = txt.as_bytes();
        let p = Parser::<DefaultElement>::new();
        assert_ok!(p.read_ply(&mut bytes));
    }
    #[test]
    fn read_property_ok() {
        let p = Parser::<DefaultElement>::new();
        let txt = "0 1 2 3";
        let mut prop = KeyMap::<PropertyDef>::new();
        prop.add(PropertyDef::new("a".to_string(), PropertyType::Scalar(ScalarType::Char)));
        prop.add(PropertyDef::new("b".to_string(), PropertyType::Scalar(ScalarType::UChar)));
        prop.add(PropertyDef::new("c".to_string(), PropertyType::Scalar(ScalarType::Short)));
        prop.add(PropertyDef::new("d".to_string(), PropertyType::Scalar(ScalarType::UShort)));
        let mut elem_def = ElementDef::new("dummy".to_string(), 0);
        elem_def.properties = prop;

        let properties = p.read_ascii_element(&txt, &elem_def);
        assert!(properties.is_ok(), format!("error: {:?}", properties));
    }
    #[test]
    fn magic_number_ok() {
        assert_ok!(g::magic_number("ply"));
    }
    #[test]
    fn magic_number_err() {
        assert_err!(g::magic_number("py"));
        assert_err!(g::magic_number("plyhi"));
        assert_err!(g::magic_number("hiply"));
    }
    #[test]
    fn format_ok() {
        assert_ok!(
            g::format("format ascii 1.0"),
            (Encoding::Ascii, Version{major: 1, minor: 0})
        );
        assert_ok!(
            g::format("format binary_big_endian 2.1"),
            (Encoding::BinaryBigEndian, Version{major: 2, minor: 1})
        );
        assert_ok!(
            g::format("format binary_little_endian 1.0"),
            (Encoding::BinaryLittleEndian, Version{major: 1, minor: 0})
        );
    }
    #[test]
    fn format_err() {
        assert_err!(g::format("format asciii 1.0"));
        assert_err!(g::format("format ascii -1.0"));
    }
    #[test]
    fn comment_ok() {
        assert_ok!(g::comment("comment hi"));
        assert_ok!(
            g::comment("comment   hi, I'm a comment!"),
            "hi, I'm a comment!"
        );
        assert_ok!(g::comment("comment "));
        assert_ok!(g::comment("comment"));
    }
    #[test]
    fn comment_err() {
        assert_err!(g::comment("commentt"));
        assert_err!(g::comment("comment hi\na comment"));
        assert_err!(g::comment("comment hi\r\na comment"));
    }
    #[test]
    fn obj_info_ok() {
        assert_ok!(g::obj_info("obj_info Hi, I can help."));
    }
    #[test]
    fn element_ok() {
        assert_ok!(
            g::element("element vertex 8"),
            ElementDef::new("vertex".to_string(), 8)
        );
    }
    #[test]
    fn element_err() {
        assert_err!(g::comment("element 8 vertex"));
    }
    #[test]
    fn property_ok() {
        assert_ok!(
            g::property("property char c"),
            PropertyDef::new("c".to_string(), PropertyType::Scalar(ScalarType::Char))
        );
    }
    #[test]
    fn property_list_ok() {
        assert_ok!(
            g::property("property list uchar int c"),
            PropertyDef::new("c".to_string(), PropertyType::List(ScalarType::UChar, ScalarType::Int))
        );
    }
    #[test]
    fn line_ok() {
        assert_ok!(g::line("ply "), Line::MagicNumber);
        assert_ok!(g::line("format ascii 1.0 "), Line::Format((Encoding::Ascii, Version{major: 1, minor: 0})));
        assert_ok!(g::line("comment a very nice comment "));
        assert_ok!(g::line("element vertex 8 "));
        assert_ok!(g::line("property float x "));
        assert_ok!(g::line("element face 6 "));
        assert_ok!(g::line("property list uchar int vertex_index "));
        assert_ok!(g::line("end_header "));
    }
    #[test]
    fn line_breaks_ok() {
        assert_ok!(g::line("ply \n"), Line::MagicNumber); // Unix, Mac OS X
        assert_ok!(g::line("ply \r"), Line::MagicNumber); // Mac pre OS X
        assert_ok!(g::line("ply \r\n"), Line::MagicNumber); // Windows
    }
    #[test]
    fn data_line_ok() {
        assert_ok!(g::data_line("-7 +5.21 \r\n"));
    }
}