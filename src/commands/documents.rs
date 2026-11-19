//! `icanotes documents list|download`: the patient's C-CDA documents as
//! `documents/v1`. Each run makes one `PatientCCD` request; a document's id
//! is the export's DocumentReference id, the same on every pull.

use std::path::{Path, PathBuf};

use pk_cli_core::private_file::{self, Replace};
use pk_cli_core::CliError;
use pk_cli_documents::{DownloadBatch, Paged, SavedDocument};

use super::{minutes, Ctx};
use crate::ccd::{self, CcdDocument};
use crate::cli::{DocumentsCmd, DownloadArgs, PatientArgs};
use crate::client::Range;

pub fn run(ctx: &Ctx, cmd: &DocumentsCmd) -> Result<(), CliError> {
    match cmd {
        DocumentsCmd::List { who } => list(ctx, who),
        DocumentsCmd::Download(args) => download(ctx, args),
    }
}

/// The patient's documents, newest first.
fn fetch(ctx: &Ctx, who: &PatientArgs) -> Result<Vec<CcdDocument>, CliError> {
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let (_, cred) = ctx.grant(&config, &profile, who)?;
    let api = ctx.api(&config, minutes(who.max_wait))?;
    let (zip, _) = api.ccd(&cred.pair(), &cred.patient_id, &Range::Complete)?;
    let archive = ccd::documents(crate::zip::read(&zip)?, &cred.patient_id);
    for why in &archive.unreadable {
        ctx.note(format!(
            "left out a file of the archive that is not a C-CDA document: {why}"
        ));
    }
    let mut docs = archive.documents;
    docs.sort_by_key(|d| std::cmp::Reverse(ccd::day(&d.header)));
    Ok(docs)
}

fn list(ctx: &Ctx, who: &PatientArgs) -> Result<(), CliError> {
    let docs = fetch(ctx, who)?;
    let rows: Vec<_> = docs.iter().map(ccd::document_row).collect();
    if ctx.json() {
        Paged::new("document", rows).emit(true);
    } else if rows.is_empty() {
        println!("ICANotes holds no C-CDA document for this patient.");
    } else {
        for r in &rows {
            println!(
                "{}  {}  {}",
                r.id,
                r.date.as_deref().unwrap_or("----------"),
                r.name
            );
        }
    }
    Ok(())
}

fn download(ctx: &Ctx, args: &DownloadArgs) -> Result<(), CliError> {
    if args.all && args.output.exists() && !args.output.is_dir() {
        return Err(CliError::Usage(format!(
            "--all writes into a directory, and {} is a file",
            args.output.display()
        )));
    }
    if let Some(id) = &args.id {
        if !pk_med_fhir::is_id(id) {
            return Err(CliError::Usage(format!("`{id}` is not a document id")));
        }
    }
    let docs = fetch(ctx, &args.who)?;
    if args.all {
        private_file::create_dir_all(&args.output)
            .map_err(|e| CliError::Other(format!("creating {}: {e}", args.output.display())))?;
        let mut saved = Vec::new();
        for d in &docs {
            saved.push(write(d, &args.output.join(ccd::file_name(d)), args.force)?);
        }
        let batch = DownloadBatch::new(args.output.display().to_string(), saved);
        pk_cli_documents::emit(&batch, ctx.json());
        return Ok(());
    }
    let id = args.id.as_deref().unwrap_or_default();
    let doc = docs.iter().find(|d| d.id == id).ok_or_else(|| {
        CliError::NotFound(format!("no C-CDA document `{id}` for this patient"))
            .with_remedy("`icanotes documents list --profile <NAME>` shows their ids")
    })?;
    let path = if args.output.is_dir() {
        args.output.join(ccd::file_name(doc))
    } else {
        args.output.clone()
    };
    let saved = write(doc, &path, args.force)?;
    pk_cli_documents::emit(&saved, ctx.json());
    Ok(())
}

/// The document's bytes, checked as XML, written owner-only.
fn write(doc: &CcdDocument, path: &Path, force: bool) -> Result<SavedDocument, CliError> {
    pk_cli_documents::verify_download(&doc.bytes, Some("xml"))?;
    let replace = if force {
        Replace::Always
    } else {
        Replace::Never
    };
    private_file::write(path, &doc.bytes, replace).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            CliError::Usage(format!("{} exists", path.display()))
                .with_remedy("pass --force to replace it")
        } else {
            CliError::Other(format!("writing {}: {e}", path.display()))
        }
    })?;
    let mut row = ccd::document_row(doc);
    row.file = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .or(row.file);
    Ok(SavedDocument::from_document(
        &row,
        PathBuf::from(path).display().to_string(),
        doc.bytes.len() as u64,
    ))
}
