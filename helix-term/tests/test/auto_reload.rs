use std::{
    fs::FileTimes,
    time::{Duration, SystemTime},
};

use helix_core::diagnostic::Severity;
use helix_view::{current_ref, editor::Action};
use tempfile::NamedTempFile;

use super::*;

// Use explicit modification times so these tests do not depend on filesystem clock resolution.
fn write_external(file: &NamedTempFile, text: &str, seconds: u64) -> anyhow::Result<()> {
    std::fs::write(file.path(), text)?;
    file.as_file().set_times(
        FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
    )?;
    Ok(())
}

async fn focus_gained(app: &mut helix_term::application::Application) {
    #[cfg(not(windows))]
    let event = termina::event::Event::FocusIn;
    #[cfg(windows)]
    let event = crossterm::event::Event::FocusGained;
    app.handle_terminal_events(Ok(event)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_clean_buffer_on_focus() -> anyhow::Result<()> {
    let file = NamedTempFile::new()?;
    write_external(&file, "original\n", 100)?;
    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;

    write_external(&file, "external edit\n", 200)?;
    focus_gained(&mut app).await;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), "external edit\n");
    assert!(!doc.is_modified());
    assert!(!doc.is_modified_on_disk());
    let version = doc.version();

    // Repeated focus events must not reload the same change again.
    focus_gained(&mut app).await;
    assert_eq!(current_ref!(app.editor).1.version(), version);
    assert_status_not_error(&app.editor);
    test_key_sequence(&mut app, None, None, false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_preserves_dirty_buffer_and_warns() -> anyhow::Result<()> {
    let file = NamedTempFile::new()?;
    write_external(&file, "original\n", 100)?;
    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .with_input_text("#[local edits|]#\n")
        .build()?;

    write_external(&file, "external edit\n", 200)?;
    focus_gained(&mut app).await;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), "local edits\n");
    assert!(doc.is_modified());
    assert_eq!(std::fs::read_to_string(file.path())?, "external edit\n");
    let (message, severity) = app.editor.get_status().expect("conflict warning");
    assert_eq!(*severity, Severity::Warning);
    assert!(message.contains("changed on disk and in Helix as well"));
    assert!(message.contains("Use :reload to discard buffer changes"));
    assert!(message.contains(file.path().file_name().unwrap().to_str().unwrap()));
    test_key_sequence(&mut app, None, None, false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_leaves_unchanged_dirty_buffer_alone() -> anyhow::Result<()> {
    let file = NamedTempFile::new()?;
    write_external(&file, "original\n", 100)?;
    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .with_input_text("#[local edits|]#\n")
        .build()?;
    let version = current_ref!(app.editor).1.version();
    app.editor.status_msg = None;

    focus_gained(&mut app).await;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.text().to_string(), "local edits\n");
    assert_eq!(doc.version(), version);
    assert!(doc.is_modified());
    assert!(app.editor.get_status().is_none());
    test_key_sequence(&mut app, None, None, false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_can_be_disabled() -> anyhow::Result<()> {
    let file = NamedTempFile::new()?;
    write_external(&file, "original\n", 100)?;
    let mut config = test_config();
    config.editor.auto_reload = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()?;

    write_external(&file, "external edit\n", 200)?;
    focus_gained(&mut app).await;
    assert_eq!(current_ref!(app.editor).1.text().to_string(), "original\n");
    test_key_sequence(&mut app, None, None, false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_hidden_clean_buffer_without_discarding_other_edits() -> anyhow::Result<()> {
    let dirty = NamedTempFile::new()?;
    let clean = NamedTempFile::new()?;
    write_external(&dirty, "original\n", 100)?;
    write_external(&clean, "original\n", 100)?;
    let mut app = AppBuilder::new()
        .with_file(dirty.path(), None)
        .with_input_text("#[local edits|]#\n")
        .build()?;
    let dirty_id = current_ref!(app.editor).1.id();
    let clean_id = app.editor.open(clean.path(), Action::Load)?;

    write_external(&dirty, "external dirty\n", 200)?;
    write_external(&clean, "external clean\n", 200)?;
    focus_gained(&mut app).await;
    assert_eq!(current_ref!(app.editor).1.id(), dirty_id);
    let clean_doc = app.editor.document(clean_id).unwrap();
    assert_eq!(clean_doc.text().to_string(), "external clean\n");
    assert!(!clean_doc.is_modified());
    let dirty_doc = app.editor.document(dirty_id).unwrap();
    assert_eq!(dirty_doc.text().to_string(), "local edits\n");
    assert!(dirty_doc.is_modified());
    assert_eq!(
        *app.editor.get_status().expect("conflict warning").1,
        Severity::Warning
    );
    test_key_sequence(&mut app, None, None, false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_ignores_scratch_and_missing_files() -> anyhow::Result<()> {
    let file = NamedTempFile::new()?;
    write_external(&file, "original\n", 100)?;
    let mut app = AppBuilder::new()
        .with_input_text("#[scratch edits|]#\n")
        .build()?;
    app.editor.open(file.path(), Action::Load)?;
    std::fs::remove_file(file.path())?;
    app.editor.status_msg = None;

    focus_gained(&mut app).await;
    assert_eq!(
        current_ref!(app.editor).1.text().to_string(),
        "scratch edits\n"
    );
    assert!(current_ref!(app.editor).1.is_modified());
    assert!(app.editor.get_status().is_none());
    test_key_sequence(&mut app, None, None, false).await
}
