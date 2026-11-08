//! Process management syscalls
use crate::{
    mm::PTEFlags,
    task::{
        change_program_brk, exit_current_and_run_next, suspend_current_and_run_next,
        with_current_memory,
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
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// Write a TimeVal through the user page table, including unaligned and cross-page buffers.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    let us = get_time_us();
    let values = [us / 1_000_000, us % 1_000_000];
    let start = ts as usize;
    with_current_memory(|memory| {
        let mut destinations = [core::ptr::null_mut(); core::mem::size_of::<TimeVal>()];
        for (offset, dst) in destinations.iter_mut().enumerate() {
            let Some(address) = start.checked_add(offset) else {
                return -1;
            };
            let Some(ptr) = memory.user_byte(address, PTEFlags::W) else {
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
    })
}

/// Read/write a user byte with permission checks, or query this task's syscall count.
pub fn sys_trace(request: usize, id: usize, data: usize) -> isize {
    match request {
        0 => with_current_memory(|memory| {
            memory
                .user_byte(id, PTEFlags::R)
                .map_or(-1, |ptr| unsafe { ptr.read() as isize })
        }),
        1 => with_current_memory(|memory| {
            let Some(ptr) = memory.user_byte(id, PTEFlags::W) else {
                return -1;
            };
            unsafe { ptr.write(data as u8) };
            0
        }),
        2 => super::trace_slot(id).map_or(0, |slot| crate::task::syscall_count(slot) as isize),
        _ => -1,
    }
}

/// Map an anonymous region at a user-selected virtual address.
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    with_current_memory(|memory| {
        if memory.map_anonymous(start, len, prot) {
            0
        } else {
            -1
        }
    })
}

/// Unmap pages previously created by mmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    with_current_memory(|memory| {
        if memory.unmap_anonymous(start, len) {
            0
        } else {
            -1
        }
    })
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
