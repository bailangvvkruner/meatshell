use super::*;

fn sftp_entry(name: &str, is_dir: bool) -> SftpEntry {
    SftpEntry {
        name: name.into(),
        full_path: format!("/{name}").into(),
        is_dir,
        size: String::new().into(),
        size_bytes: 0.0,
        modified: String::new().into(),
        modified_ts: 0.0,
        mode: 0,
        selected: false,
    }
}

fn sftp_names(entries: &[SftpEntry]) -> Vec<String> {
    entries.iter().map(|e| e.name.to_string()).collect()
}

#[test]
fn sftp_name_sort_uses_natural_numeric_order() {
    let mut entries = vec![
        sftp_entry("file100", false),
        sftp_entry("file10", false),
        sftp_entry("file2", false),
        sftp_entry("file11", false),
        sftp_entry("file1", false),
    ];
    sort_sftp_entries(&mut entries, "name", 1);
    assert_eq!(
        sftp_names(&entries),
        vec!["file1", "file2", "file10", "file11", "file100"]
    );

    sort_sftp_entries(&mut entries, "name", -1);
    assert_eq!(
        sftp_names(&entries),
        vec!["file100", "file11", "file10", "file2", "file1"]
    );
}

#[test]
fn sftp_default_sort_keeps_dirs_first_with_natural_names() {
    let mut entries = vec![
        sftp_entry("file100", false),
        sftp_entry("dir10", true),
        sftp_entry("file11", false),
        sftp_entry("dir2", true),
    ];
    sort_sftp_entries(&mut entries, "", 0);
    assert_eq!(
        sftp_names(&entries),
        vec!["dir2", "dir10", "file11", "file100"]
    );
}
