//! File and filesystem-related syscalls
use crate::fs::{open_file, OpenFlags, Stat, ROOT_INODE};
use crate::mm::{translated_byte_buffer, PTEFlags, UserBuffer};
use crate::task::{current_task, current_user_token};
use alloc::string::String;
use core::{mem::size_of, str};
use easy_fs::NAME_LENGTH_LIMIT;

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_write", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_read", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        trace!("kernel: sys_read .. file.read");
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    trace!("kernel:pid[{}] sys_open", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let (Some(path), Some(flags)) = (read_file_name(path), OpenFlags::from_bits(flags)) else {
        return -1;
    };
    if let Some(inode) = open_file(path.as_str(), flags) {
        let mut inner = task.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_close", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

pub(super) fn read_file_name(ptr: *const u8) -> Option<String> {
    let mut name = [0; NAME_LENGTH_LIMIT + 1];
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    let start = ptr as usize;
    for (index, byte) in name.iter_mut().enumerate() {
        let addr = start.checked_add(index)?;
        let src = inner.memory_set.user_byte(addr, PTEFlags::R)?;
        *byte = unsafe { src.read() };
        if *byte == 0 {
            return str::from_utf8(&name[..index]).ok().map(String::from);
        }
    }
    None
}

pub fn sys_fstat(fd: usize, st: *mut Stat) -> isize {
    let task = current_task().unwrap();
    let stat = task
        .inner_exclusive_access()
        .fd_table
        .get(fd)
        .and_then(|entry| entry.as_ref())
        .and_then(|file| file.stat());
    let Some(stat) = stat else {
        return -1;
    };

    let mut bytes = [0; size_of::<Stat>()];
    bytes[..8].copy_from_slice(&stat.dev.to_ne_bytes());
    bytes[8..16].copy_from_slice(&stat.ino.to_ne_bytes());
    bytes[16..20].copy_from_slice(&stat.mode.bits().to_ne_bytes());
    bytes[20..24].copy_from_slice(&stat.nlink.to_ne_bytes());
    super::copy_to_user(st.cast(), &bytes)
}

pub fn sys_linkat(old_name: *const u8, new_name: *const u8) -> isize {
    let (Some(old_name), Some(new_name)) = (read_file_name(old_name), read_file_name(new_name))
    else {
        return -1;
    };
    ROOT_INODE.link(&old_name, &new_name).map_or(-1, |_| 0)
}

pub fn sys_unlinkat(name: *const u8) -> isize {
    let Some(name) = read_file_name(name) else {
        return -1;
    };
    ROOT_INODE.unlink(&name).map_or(-1, |_| 0)
}
