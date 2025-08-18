use shared_memory::ShmemConf;
use std::path::Path;

#[test]
fn tmpfs_create_new() {
    let mut s = ShmemConf::new()
        .size(4090)
        .use_tmpfs_with_dir("/tmp")
        .create()
        .unwrap();

    assert!(s.is_owner());
    assert!(!s.get_os_id().is_empty());
    assert!(s.len() >= 4090);
    assert!(!s.as_ptr().is_null());

    // Check that the unique_id is a file path (contains '/')
    assert!(s.get_os_id().contains('/'));

    unsafe {
        assert_eq!(s.as_slice().len(), s.len());
        assert_eq!(s.as_slice_mut().len(), s.len());
    }
}

#[test]
fn tmpfs_create_with_os_id() {
    let os_id = "test_tmpfs_id";
    let mut s = ShmemConf::new()
        .size(4090)
        .use_tmpfs_with_dir("/tmp")
        .os_id(os_id)
        .create()
        .unwrap();

    assert!(s.is_owner());
    assert!(s.get_os_id().contains(os_id));
    assert!(s.len() >= 4090);
    assert!(!s.as_ptr().is_null());

    unsafe {
        assert_eq!(s.as_slice().len(), s.len());
        assert_eq!(s.as_slice_mut().len(), s.len());
    }
}

#[test]
fn tmpfs_create_with_custom_dir() {
    let custom_dir = "/tmp";
    let mut s = ShmemConf::new()
        .size(4090)
        .use_tmpfs_with_dir(custom_dir)
        .create()
        .unwrap();

    assert!(s.is_owner());
    assert!(s.get_os_id().starts_with(custom_dir));
    assert!(s.len() >= 4090);
    assert!(!s.as_ptr().is_null());

    unsafe {
        assert_eq!(s.as_slice().len(), s.len());
        assert_eq!(s.as_slice_mut().len(), s.len());
    }
}

#[test]
fn tmpfs_create_with_flink() {
    let flink = Path::new("tmpfs_flink_test");

    let mut s = ShmemConf::new()
        .flink(flink)
        .size(4090)
        .use_tmpfs_with_dir("/tmp")
        .create()
        .unwrap();

    assert!(s.is_owner());
    assert!(!s.get_os_id().is_empty());
    assert!(flink.is_file());
    assert!(s.len() >= 4090);
    assert!(!s.as_ptr().is_null());

    unsafe {
        assert_eq!(s.as_slice().len(), s.len());
        assert_eq!(s.as_slice_mut().len(), s.len());
    }

    drop(s);

    assert!(!flink.is_file());
}

#[test]
fn tmpfs_open_os_id() {
    let os_id = "test_tmpfs_open_id";
    let s1 = ShmemConf::new()
        .size(4090)
        .use_tmpfs_with_dir("/tmp")
        .os_id(os_id)
        .create()
        .unwrap();

    // Open with the same os_id
    let mut s2 = ShmemConf::new()
        .use_tmpfs_with_dir("/tmp")
        .os_id(os_id)
        .open()
        .unwrap();

    assert!(!s2.is_owner());
    assert!(s2.get_os_id().contains(os_id));
    assert!(s2.len() >= 4090);
    assert!(!s2.as_ptr().is_null());

    unsafe {
        assert_eq!(s2.as_slice().len(), s2.len());
        assert_eq!(s2.as_slice_mut().len(), s2.len());
    }

    // Drop the owner of the mapping
    drop(s1);

    // Make sure it cannot be opened again after owner is dropped
    assert!(ShmemConf::new()
        .use_tmpfs_with_dir("/tmp")
        .os_id(os_id)
        .open()
        .is_err());

    drop(s2);
}

#[test]
fn tmpfs_open_flink() {
    let flink = Path::new("tmpfs_open_flink_test");
    let s1 = ShmemConf::new()
        .flink(flink)
        .size(4090)
        .use_tmpfs_with_dir("/tmp")
        .create()
        .unwrap();

    // Open with file link
    let mut s2 = ShmemConf::new()
        .flink(flink)
        .use_tmpfs_with_dir("/tmp")
        .open()
        .unwrap();

    assert!(!s2.is_owner());
    assert!(!s2.get_os_id().is_empty());
    assert!(flink.is_file());
    assert!(s2.len() >= 4090);
    assert!(!s2.as_ptr().is_null());

    unsafe {
        assert_eq!(s2.as_slice().len(), s2.len());
        assert_eq!(s2.as_slice_mut().len(), s2.len());
    }

    // Drop the owner of the mapping
    drop(s1);

    // Make sure it cannot be opened again
    assert!(ShmemConf::new()
        .flink(flink)
        .use_tmpfs_with_dir("/tmp")
        .open()
        .is_err());

    drop(s2);
}

#[test]
fn tmpfs_share_data() {
    let os_id = "test_tmpfs_share_data";
    let s1 = ShmemConf::new()
        .size(core::mem::size_of::<u32>())
        .use_tmpfs_with_dir("/tmp")
        .os_id(os_id)
        .create()
        .unwrap();

    // Open with the same os_id
    let s2 = ShmemConf::new()
        .use_tmpfs_with_dir("/tmp")
        .os_id(os_id)
        .open()
        .unwrap();

    let ptr1 = s1.as_ptr() as *mut u32;
    let ptr2 = s2.as_ptr() as *mut u32;

    // Confirm that the two pointers are different
    assert_ne!(ptr1, ptr2);

    // Write a value from s1 and read it from s2
    unsafe {
        let shared_val = 0xBADC0FEE;
        ptr1.write_volatile(shared_val);
        let read_val = ptr2.read_volatile();
        assert_eq!(read_val, shared_val);
    }
}

#[test]
fn tmpfs_mixed_with_traditional() {
    // Test that tmpfs mode doesn't interfere with traditional mode

    // Create traditional mapping
    let s1 = ShmemConf::new()
        .size(4090)
        .create() // No tmpfs
        .unwrap();

    // Create tmpfs mapping
    let s2 = ShmemConf::new()
        .size(4090)
        .use_tmpfs_with_dir("/tmp")
        .create()
        .unwrap();

    // Both should work independently
    assert!(s1.is_owner());
    assert!(s2.is_owner());
    assert_ne!(s1.get_os_id(), s2.get_os_id());

    // Traditional ID should start with '/' and have only one '/'
    assert!(s1.get_os_id().starts_with('/'));
    assert_eq!(s1.get_os_id().matches('/').count(), 1);

    // tmpfs ID should be a file path with multiple '/' or be in tmpfs directory
    assert!(
        s2.get_os_id().contains("shmem_")
            || s2.get_os_id().contains("/dev/shm")
            || s2.get_os_id().contains("/tmp")
    );
}

#[test]
fn tmpfs_os_id_with_flink() {
    let flink = Path::new("tmpfs_os_id_flink_test");
    let os_id = "test_tmpfs_combined";

    let s1 = ShmemConf::new()
        .size(4090)
        .use_tmpfs_with_dir("/tmp")
        .os_id(os_id)
        .flink(flink)
        .create()
        .unwrap();

    assert!(s1.is_owner());
    assert!(s1.get_os_id().contains(os_id));
    assert!(flink.is_file());

    // Should be able to open via os_id
    let s2 = ShmemConf::new()
        .use_tmpfs_with_dir("/tmp")
        .os_id(os_id)
        .open()
        .unwrap();

    // Should be able to open via flink
    let s3 = ShmemConf::new()
        .use_tmpfs_with_dir("/tmp")
        .flink(flink)
        .open()
        .unwrap();

    assert!(!s2.is_owner());
    assert!(!s3.is_owner());
    assert_eq!(s2.get_os_id(), s3.get_os_id());

    drop(s1);

    // Cleanup
    drop(s2);
    drop(s3);
}
