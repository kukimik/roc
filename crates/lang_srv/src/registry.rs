use log::{debug, info, trace, warn};

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use tokio::sync::{Mutex, MutexGuard};

use tower_lsp::lsp_types::{
    CompletionResponse, Diagnostic, GotoDefinitionResponse, Hover, Position, SemanticTokensResult,
    TextEdit, Url,
};

use crate::analysis::{AnalyzedDocument, DocInfo};

#[derive(Debug)]
pub(crate) struct DocumentPair {
    info: DocInfo,
    latest_document: OnceLock<Arc<AnalyzedDocument>>,
    last_good_document: Arc<AnalyzedDocument>,
}

impl DocumentPair {
    pub(crate) fn new(
        latest_doc: Arc<AnalyzedDocument>,
        last_good_document: Arc<AnalyzedDocument>,
    ) -> Self {
        Self {
            info: latest_doc.doc_info.clone(),
            latest_document: OnceLock::from(latest_doc),
            last_good_document,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Registry {
    documents: Mutex<HashMap<Url, DocumentPair>>,
}

impl Registry {
    pub async fn get_latest_version(&self, url: &Url) -> Option<i32> {
        self.documents
            .lock()
            .await
            .get(&url)
            .map(|x| x.info.version)
    }

    fn update_document<'a>(
        documents: &mut MutexGuard<'a, HashMap<Url, DocumentPair>>,
        document: Arc<AnalyzedDocument>,
    ) {
        let url = document.url().clone();
        match documents.get_mut(&url) {
            Some(old_doc) => {
                if document.type_checked() {
                    *old_doc = DocumentPair::new(document.clone(), document);
                } else {
                    debug!(
                        "Document typechecking failed at version {:?}, not updating last_good_document",
                        &document.doc_info.version
                    );
                    *old_doc = DocumentPair::new(document, old_doc.last_good_document.clone());
                }
            }
            None => {
                documents.insert(url.clone(), DocumentPair::new(document.clone(), document));
            }
        }
    }

    pub async fn apply_changes<'a>(&self, analysed_docs: Vec<AnalyzedDocument>, updating_url: Url) {
        let mut documents = self.documents.lock().await;
        debug!(
            "finised doc analysis for doc: {:?}",
            updating_url.to_string()
        );

        for document in analysed_docs {
            let document = Arc::new(document);
            //Write the newly analysed document into the partial document that any request requiring the latest document will be waiting on
            if document.doc_info.url == updating_url {
                documents
                    .get_mut(&updating_url)
                    .map(|a| a.latest_document.set(document.clone()).unwrap());
            }
            Registry::update_document(&mut documents, document);
        }
    }

    pub async fn apply_doc_info_changes(&self, url: Url, info: DocInfo) {
        let mut documents_lock = self.documents.lock().await;
        let doc = documents_lock.get_mut(&url);
        match doc {
            Some(a) => {
                debug!(
                    "set the docInfo for {:?} to version:{:?}",
                    url.as_str(),
                    info.version
                );
                *a = DocumentPair {
                    info,
                    last_good_document: a.last_good_document.clone(),
                    latest_document: OnceLock::new(),
                };
            }
            None => debug!("no existing docinfo for {:?} ", url.as_str()),
        }
    }

    async fn document_info_by_url(&self, url: &Url) -> Option<DocInfo> {
        self.documents.lock().await.get(url).map(|a| a.info.clone())
    }

    ///Tries to get the latest document from analysis.
    ///Gives up and returns none after 5 seconds.
    async fn latest_document_by_url(&self, url: &Url) -> Option<Arc<AnalyzedDocument>> {
        let start = std::time::Instant::now();
        let duration = std::time::Duration::from_secs(5);

        while start.elapsed() < duration {
            match self.documents.lock().await.get(url) {
                Some(a) => match a.latest_document.get() {
                    Some(a) => return Some(a.clone()),
                    None => (),
                },

                None => return None,
            }
        }
        warn!("Timed out tring to get latest document");
        None
    }

    pub async fn diagnostics(&self, url: &Url) -> Vec<Diagnostic> {
        let Some( document) = self.latest_document_by_url(url).await else {
            return vec![];

        };
        document.diagnostics()
    }

    pub async fn hover(&self, url: &Url, position: Position) -> Option<Hover> {
        self.latest_document_by_url(url).await?.hover(position)
    }

    pub async fn goto_definition(
        &self,
        url: &Url,
        position: Position,
    ) -> Option<GotoDefinitionResponse> {
        let document = self.latest_document_by_url(url).await?;
        let symbol = document.symbol_at(position)?;
        let def_document_url = document.module_url(symbol.module_id())?;
        let def_document = self.latest_document_by_url(&def_document_url).await?;
        def_document.definition(symbol)
    }

    pub async fn formatting(&self, url: &Url) -> Option<Vec<TextEdit>> {
        let document = self.document_info_by_url(url).await?;
        document.format()
    }

    pub async fn semantic_tokens(&self, url: &Url) -> Option<SemanticTokensResult> {
        let document = self.document_info_by_url(url).await?;
        document.semantic_tokens()
    }
    pub async fn completion_items(
        &self,
        url: &Url,
        position: Position,
    ) -> Option<CompletionResponse> {
        trace!("starting completion ");
        let lock = self.documents.lock().await;
        let pair = lock.get(url)?;

        let latest_doc_info = &pair.info;
        info!(
            "using document version:{:?} for completion ",
            latest_doc_info.version
        );

        let completions = pair
            .last_good_document
            .completion_items(position, &latest_doc_info)?;

        Some(CompletionResponse::Array(completions))
    }
}
