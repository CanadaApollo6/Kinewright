//! Standalone Linux protocol fixture. Owns and removes only its disposable IPC objects.
use std::{
    ffi::{CString, c_char, c_int, c_long, c_void},
    io::{self, BufRead, Write},
    ptr,
};

unsafe extern "C" {
    fn shmget(key: c_int, size: usize, flags: c_int) -> c_int;
    fn shmat(id: c_int, address: *const c_void, flags: c_int) -> *mut c_void;
    fn shmdt(address: *const c_void) -> c_int;
    fn shmctl(id: c_int, command: c_int, buffer: *mut c_void) -> c_int;
    fn msgget(key: c_int, flags: c_int) -> c_int;
    fn msgsnd(id: c_int, message: *const c_void, size: usize, flags: c_int) -> c_int;
    fn msgrcv(id: c_int, message: *mut c_void, size: usize, kind: c_long, flags: c_int) -> isize;
    fn msgctl(id: c_int, command: c_int, buffer: *mut c_void) -> c_int;
    fn semget(key: c_int, count: c_int, flags: c_int) -> c_int;
    fn semctl(id: c_int, number: c_int, command: c_int, ...) -> c_int;
    fn semop(id: c_int, operations: *mut Operation, count: usize) -> c_int;
    fn shm_open(name: *const c_char, flags: c_int, mode: c_int) -> c_int;
    fn shm_unlink(name: *const c_char) -> c_int;
    fn mq_open(name: *const c_char, flags: c_int, ...) -> c_int;
    fn mq_send(queue: c_int, message: *const c_char, size: usize, priority: u32) -> c_int;
    fn mq_receive(queue: c_int, message: *mut c_char, size: usize, priority: *mut u32) -> isize;
    fn mq_close(queue: c_int) -> c_int;
    fn mq_unlink(name: *const c_char) -> c_int;
    fn close(fd: c_int) -> c_int;
}
#[repr(C)]
struct Operation {
    number: u16,
    operation: i16,
    flags: i16,
}
#[repr(C)]
struct Message {
    kind: c_long,
    byte: u8,
}
struct Objects {
    memory: c_int,
    pointer: *mut c_void,
    messages: c_int,
    semaphore: c_int,
    queue: c_int,
    name: CString,
    shared: c_int,
}
impl Drop for Objects {
    fn drop(&mut self) {
        // SAFETY: All identifiers and the mapping belong exclusively to this fixture.
        unsafe {
            if self.pointer as isize != -1 {
                shmdt(self.pointer);
            }
            if self.memory >= 0 {
                shmctl(self.memory, 0, ptr::null_mut());
            }
            if self.messages >= 0 {
                msgctl(self.messages, 0, ptr::null_mut());
            }
            if self.semaphore >= 0 {
                semctl(self.semaphore, 0, 0);
            }
            if self.queue >= 0 {
                mq_close(self.queue);
                mq_unlink(self.name.as_ptr());
            }
            if self.shared >= 0 {
                close(self.shared);
                shm_unlink(self.name.as_ptr());
            }
        }
    }
}
fn serve() {
    let name = CString::new(format!("/kinewright-c2-fix1-{}", std::process::id())).unwrap();
    let mut objects = Objects {
        memory: -1,
        pointer: -1_isize as *mut c_void,
        messages: -1,
        semaphore: -1,
        queue: -1,
        shared: -1,
        name,
    };
    // SAFETY: Fresh IPC_PRIVATE IDs and O_EXCL names are owned by Objects on every exit.
    unsafe {
        objects.memory = shmget(0, 4096, 0o600);
        assert!(objects.memory >= 0);
        objects.pointer = shmat(objects.memory, ptr::null(), 0);
        assert_ne!(objects.pointer as isize, -1);
        *objects.pointer.cast::<u8>() = b'S';
        objects.messages = msgget(0, 0o600);
        assert!(objects.messages >= 0);
        let message = Message {
            kind: 1,
            byte: b'M',
        };
        assert_eq!(
            msgsnd(objects.messages, (&message as *const Message).cast(), 1, 0),
            0
        );
        objects.semaphore = semget(0, 1, 0o600);
        assert!(objects.semaphore >= 0);
        assert_eq!(semctl(objects.semaphore, 0, 16, 1_i32), 0);
        objects.queue = mq_open(
            objects.name.as_ptr(),
            0o100 | 0o200 | 0o4000 | 2,
            0o600_i32,
            ptr::null::<c_void>(),
        );
        assert!(
            objects.queue >= 0,
            "host mq_open: {}",
            io::Error::last_os_error()
        );
        assert_eq!(mq_send(objects.queue, c"Q".as_ptr(), 1, 0), 0);
        objects.shared = shm_open(objects.name.as_ptr(), 0o100 | 0o200 | 2, 0o600);
        assert!(objects.shared >= 0);
    }
    let path = format!("/dev/shm{}", objects.name.to_str().unwrap());
    std::fs::write(&path, b"host-private-posix-shm").unwrap();
    println!(
        "{} {} {} {}",
        objects.memory,
        objects.messages,
        objects.semaphore,
        objects.name.to_str().unwrap()
    );
    io::stdout().flush().unwrap();
    let mut stop = String::new();
    io::stdin().lock().read_line(&mut stop).unwrap();
    // SAFETY: The retained mapping/IDs are our own; failed lookups only mean the probe tampered.
    let intact = unsafe {
        let mut message = Message { kind: 0, byte: 0 };
        let mut queue = [0_i8; 8192];
        *objects.pointer.cast::<u8>() == b'S'
            && msgrcv(
                objects.messages,
                (&mut message as *mut Message).cast(),
                1,
                0,
                0o4000,
            ) == 1
            && message.byte == b'M'
            && semctl(objects.semaphore, 0, 12) == 1
            && mq_receive(
                objects.queue,
                queue.as_mut_ptr(),
                queue.len(),
                ptr::null_mut(),
            ) == 1
            && queue[0] == b'Q' as i8
            && std::fs::read(path).is_ok_and(|value| value == b"host-private-posix-shm")
    };
    println!("intact={intact}");
}
fn probe(args: &[String]) {
    let memory: c_int = args[2].parse().unwrap();
    let messages: c_int = args[3].parse().unwrap();
    let semaphore: c_int = args[4].parse().unwrap();
    let name = CString::new(args[5].as_str()).unwrap();
    let mut allowed = Vec::new();
    let mut sysv_eperm = true;
    let mut record = |name: &str, result: isize, sysv: bool| {
        let success = result >= 0;
        if sysv {
            sysv_eperm &= !success && io::Error::last_os_error().raw_os_error() == Some(1);
        }
        allowed.push(format!("\"{name}\":{success}"));
    };
    // SAFETY: The controller supplied only this fixture's disposable host IDs/names.
    // Reads/writes/removals are intentional adversarial operations on those objects.
    unsafe {
        let mapping = shmat(memory, ptr::null(), 0);
        record("shmat", if mapping as isize == -1 { -1 } else { 0 }, true);
        if mapping as isize != -1 {
            let _observed = *mapping.cast::<u8>();
            *mapping.cast::<u8>() = b'X';
            shmdt(mapping);
        }
        record(
            "shm_remove",
            shmctl(memory, 0, ptr::null_mut()) as isize,
            true,
        );
        let mut message = Message {
            kind: 1,
            byte: b'X',
        };
        record(
            "msg_read",
            msgrcv(
                messages,
                (&mut message as *mut Message).cast(),
                1,
                0,
                0o4000,
            ),
            true,
        );
        record(
            "msg_write",
            msgsnd(messages, (&message as *const Message).cast(), 1, 0o4000) as isize,
            true,
        );
        record(
            "msg_remove",
            msgctl(messages, 0, ptr::null_mut()) as isize,
            true,
        );
        record("sem_read", semctl(semaphore, 0, 12) as isize, true);
        let mut operation = Operation {
            number: 0,
            operation: 1,
            flags: 0o4000,
        };
        record(
            "sem_alter",
            semop(semaphore, &mut operation, 1) as isize,
            true,
        );
        record("sem_set", semctl(semaphore, 0, 16, 2_i32) as isize, true);
        record("sem_remove", semctl(semaphore, 0, 0) as isize, true);
        let queue = mq_open(name.as_ptr(), 2 | 0o4000);
        record("mq_open", queue as isize, false);
        if queue >= 0 {
            let mut bytes = [0_i8; 8192];
            record(
                "mq_read",
                mq_receive(queue, bytes.as_mut_ptr(), bytes.len(), ptr::null_mut()),
                false,
            );
            record(
                "mq_write",
                mq_send(queue, c"X".as_ptr(), 1, 0) as isize,
                false,
            );
            mq_close(queue);
        }
        record("mq_remove", mq_unlink(name.as_ptr()) as isize, false);
        let shared = shm_open(name.as_ptr(), 2, 0);
        record("posix_shm_open", shared as isize, false);
        if shared >= 0 {
            close(shared);
        }
        record(
            "posix_shm_remove",
            shm_unlink(name.as_ptr()) as isize,
            false,
        );
    }
    for path in [
        format!("/dev/shm{}", name.to_str().unwrap()),
        format!("/dev/mqueue{}", name.to_str().unwrap()),
    ] {
        allowed.push(format!(
            "\"path_read_{}\":{}",
            allowed.len(),
            std::fs::read(&path).is_ok()
        ));
        allowed.push(format!(
            "\"path_write_{}\":{}",
            allowed.len(),
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|mut file| file.write_all(b"X"))
                .is_ok()
        ));
        allowed.push(format!(
            "\"path_remove_{}\":{}",
            allowed.len(),
            std::fs::remove_file(&path).is_ok()
        ));
    }
    allowed.push(format!("\"sysv_eperm\":{sysv_eperm}"));
    let path = std::path::Path::new(&args[6]);
    std::fs::write(
        path.with_extension("partial"),
        format!("{{{}}}", allowed.join(",")),
    )
    .unwrap();
    std::fs::rename(path.with_extension("partial"), path).unwrap();
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args[1].as_str() {
        "serve" => serve(),
        "probe" => probe(&args),
        _ => panic!("unknown fixture role"),
    }
}
