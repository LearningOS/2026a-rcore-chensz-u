//! Process management syscalls
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::{
    loader::{get_app_data_by_name, max_app_name_len},
    mm::{translated_refmut, PTEFlags},
    task::{
        add_task, current_task, exit_current_and_run_next, suspend_current_and_run_next,
        TaskControlBlock,
    },
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel:pid[{}] sys_yield", current_task().unwrap().pid.0);
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    if let Some(data) = app_from_user_path(&task, path) {
        task.exec(data);
        0
    } else {
        -1
    }
}

fn app_from_user_path(task: &TaskControlBlock, path: *const u8) -> Option<&'static [u8]> {
    let inner = task.inner_exclusive_access();
    let mut name = Vec::new();
    for offset in 0..=max_app_name_len() {
        let address = (path as usize).checked_add(offset)?;
        let byte = inner.memory_set.user_byte(address, PTEFlags::R)?;
        let byte = unsafe { byte.read() };
        if byte == 0 {
            return get_app_data_by_name(core::str::from_utf8(&name).ok()?);
        }
        name.push(byte);
    }
    None
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    trace!(
        "kernel::pid[{}] sys_waitpid [{}]",
        current_task().unwrap().pid.0,
        pid
    );
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// Write the current time into a user buffer, including buffers that cross pages.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    let us = get_time_us();
    let values = [us / 1_000_000, us % 1_000_000];
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    let mut destinations = [core::ptr::null_mut(); core::mem::size_of::<TimeVal>()];
    for (offset, dst) in destinations.iter_mut().enumerate() {
        let Some(address) = (ts as usize).checked_add(offset) else {
            return -1;
        };
        let Some(ptr) = inner.memory_set.user_byte(address, PTEFlags::W) else {
            return -1;
        };
        *dst = ptr;
    }
    for (dst, byte) in destinations
        .into_iter()
        .zip(values.into_iter().flat_map(usize::to_ne_bytes))
    {
        unsafe { dst.write(byte) };
    }
    0
}

/// Map anonymous user memory in the current process.
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if inner.memory_set.map_anonymous(start, len, prot) {
        0
    } else {
        -1
    }
}

/// Unmap anonymous user memory in the current process.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if inner.memory_set.unmap_anonymous(start, len) {
        0
    } else {
        -1
    }
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// Create and schedule a child that runs the named application.
pub fn sys_spawn(path: *const u8) -> isize {
    let parent = current_task().unwrap();
    let Some(data) = app_from_user_path(&parent, path) else {
        return -1;
    };
    let child = parent.spawn(data);
    let pid = child.getpid();
    add_task(child);
    pid as isize
}

/// Set the scheduling priority of the current process.
pub fn sys_set_priority(prio: isize) -> isize {
    if prio < 2 {
        return -1;
    }
    current_task().unwrap().inner_exclusive_access().priority = prio as u64;
    prio
}
