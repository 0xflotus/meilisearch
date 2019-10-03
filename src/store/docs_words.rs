use std::sync::Arc;
use rkv::Value;
use crate::DocumentId;

#[derive(Copy, Clone)]
pub struct DocsWords {
    pub(crate) docs_words: rkv::SingleStore,
}

impl DocsWords {
    pub fn put_doc_words(
        &self,
        writer: &mut rkv::Writer,
        document_id: DocumentId,
        words: &fst::Set,
    ) -> Result<(), rkv::StoreError>
    {
        let document_id_bytes = document_id.0.to_be_bytes();
        let bytes = words.as_fst().as_bytes();
        self.docs_words.put(writer, document_id_bytes, &Value::Blob(bytes))
    }

    pub fn del_doc_words(
        &self,
        writer: &mut rkv::Writer,
        document_id: DocumentId,
    ) -> Result<(), rkv::StoreError>
    {
        let document_id_bytes = document_id.0.to_be_bytes();
        self.docs_words.delete(writer, document_id_bytes)
    }

    pub fn doc_words<T: rkv::Readable>(
        &self,
        reader: &T,
        document_id: DocumentId,
    ) -> Result<Option<fst::Set>, rkv::StoreError>
    {
        let document_id_bytes = document_id.0.to_be_bytes();
        match self.docs_words.get(reader, document_id_bytes)? {
            Some(Value::Blob(bytes)) => {
                let len = bytes.len();
                let bytes = Arc::from(bytes);
                let fst = fst::raw::Fst::from_shared_bytes(bytes, 0, len).unwrap();
                Ok(Some(fst::Set::from(fst)))
            },
            Some(value) => panic!("invalid type {:?}", value),
            None => Ok(None),
        }
    }
}
