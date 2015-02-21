//! Support for Unix domain socket clients and servers.
#![feature(io, std_misc, path, core)]
#![warn(missing_docs)]
#![doc(html_root_url="https://sfackler.github.io/rust-unix-socket/doc/unix_socket")]

extern crate libc;

use std::cmp;
use std::ffi::AsOsStr;
use std::io;
use std::iter::IntoIterator;
use std::mem;
use std::num::Int;
use std::os::unix::{Fd, OsStrExt, AsRawFd};
use std::path::AsPath;

struct Inner(Fd);

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

impl Inner {
    unsafe fn new() -> io::Result<Inner> {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Inner(fd))
        }
    }

}

unsafe fn sockaddr_un<P: AsPath + ?Sized>(path: &P) -> io::Result<libc::sockaddr_un> {
    let mut addr: libc::sockaddr_un = mem::zeroed();
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;

    let bytes = path.as_path().as_os_str().as_bytes();
    if bytes.len() > addr.sun_path.len() - 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
                                  "path must be smaller than SUN_LEN",
                                  None));
    }
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes.iter()) {
        *dst = *src as libc::c_char;
    }
    // null byte's already there because we zeroed the struct

    Ok(addr)
}

/// A stream which communicates over a Unix domain socket.
pub struct UnixStream {
    inner: Inner,
}

impl UnixStream {
    /// Connect to the socket named by `path`.
    pub fn connect<P: AsPath + ?Sized>(path: &P) -> io::Result<UnixStream> {
        unsafe {
            let inner = try!(Inner::new());
            let addr = try!(sockaddr_un(path));

            let ret = libc::connect(inner.0,
                                    &addr as *const _ as *const _,
                                    mem::size_of::<libc::sockaddr_un>() as libc::socklen_t);
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(UnixStream {
                    inner: inner,
                })
            }
        }
    }
}

fn calc_len(buf: &[u8]) -> libc::size_t {
    cmp::min(<libc::size_t as Int>::max_value() as usize, buf.len()) as libc::size_t
}

impl io::Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let ret = unsafe {
            libc::recv(self.inner.0, buf.as_mut_ptr() as *mut _, calc_len(buf), 0)
        };

        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }
}

impl io::Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let ret = unsafe {
            libc::send(self.inner.0, buf.as_ptr() as *const _, calc_len(buf), 0)
        };

        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AsRawFd for UnixStream {
    fn as_raw_fd(&self) -> Fd {
        self.inner.0
    }
}

/// A structure representing a Unix domain socket server.
pub struct UnixListener {
    inner: Inner,
}

impl UnixListener {
    /// Creates a new `UnixListener` which will be bound to the specified socket.
    pub fn bind<P: AsPath + ?Sized>(path: &P) -> io::Result<UnixListener> {
        unsafe {
            let inner = try!(Inner::new());
            let addr = try!(sockaddr_un(path));

            let ret = libc::bind(inner.0,
                                 &addr as *const _ as *const _,
                                 mem::size_of::<libc::sockaddr_un>() as libc::socklen_t);
            if ret < 0 {
                return Err(io::Error::last_os_error());
            }

            let ret = libc::listen(inner.0, 128);
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(UnixListener {
                    inner: inner,
                })
            }
        }
    }

    /// Accepts a new incoming connection to this listener.
    pub fn accept(&self) -> io::Result<UnixStream> {
        unsafe {
            let ret = libc::accept(self.inner.0, 0 as *mut _, 0 as *mut _);
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(UnixStream {
                    inner: Inner(ret)
                })
            }
        }
    }

    /// Returns an iterator over incoming connections.
    ///
    /// The iterator will never return `None`.
    pub fn incoming<'a>(&'a self) -> Incoming<'a> {
        Incoming {
            listener: self
        }
    }
}

impl AsRawFd for UnixListener {
    fn as_raw_fd(&self) -> Fd {
        self.inner.0
    }
}

impl<'a> IntoIterator for &'a UnixListener {
    type Item = io::Result<UnixStream>;
    type IntoIter = Incoming<'a>;

    fn into_iter(self) -> Incoming<'a> {
        self.incoming()
    }
}

/// An iterator over incoming connections to a `UnixListener`.
///
/// It will never return `None`.
pub struct Incoming<'a> {
    listener: &'a UnixListener,
}

impl<'a> Iterator for Incoming<'a> {
    type Item = io::Result<UnixStream>;

    fn next(&mut self) -> Option<io::Result<UnixStream>> {
        Some(self.listener.accept())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (Int::max_value(), None)
    }
}

#[cfg(test)]
mod test {
    extern crate temporary;

    use std::thread;
    use std::io;
    use std::io::prelude::*;

    use {UnixListener, UnixStream};

    macro_rules! or_panic {
        ($e:expr) => {
            match $e {
                Ok(e) => e,
                Err(e) => panic!("{}", e),
            }
        }
    }

    #[test]
    fn basic() {
        let dir = or_panic!(temporary::Directory::new("unix_socket"));
        let socket_path = dir.path().join("sock");
        let msg1 = b"hello";
        let msg2 = b"world!";

        let listener = or_panic!(UnixListener::bind(&socket_path));
        let thread = thread::scoped(|| {
            let mut stream = or_panic!(listener.accept());
            let mut buf = [0; 5];
            or_panic!(stream.read(&mut buf));
            assert_eq!(msg1, buf);
            or_panic!(stream.write_all(msg2));
        });

        let mut stream = or_panic!(UnixStream::connect(&socket_path));
        or_panic!(stream.write_all(msg1));
        let mut buf = vec![];
        or_panic!(stream.read_to_end(&mut buf));
        assert_eq!(msg2, buf);
        drop(stream);

        thread.join();
    }

    #[test]
    fn iter() {
        let dir = or_panic!(temporary::Directory::new("unix_socket"));
        let socket_path = dir.path().join("sock");

        let listener = or_panic!(UnixListener::bind(&socket_path));
        let thread = thread::scoped(|| {
            for stream in listener.incoming().take(2) {
                let mut stream = or_panic!(stream);
                let mut buf = [0];
                or_panic!(stream.read(&mut buf));
            }
        });

        for _ in 0..2 {
            let mut stream = or_panic!(UnixStream::connect(&socket_path));
            or_panic!(stream.write_all(&[0]));
        }

        thread.join();
    }

    #[test]
    fn long_path() {
        let socket_path = "asdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasd\
                           asdfasdfasdfadfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasd";
        match UnixStream::connect(socket_path) {
            Err(ref e) if e.kind() == io::ErrorKind::InvalidInput => {}
            Err(e) => panic!("unexpected error {}", e),
            Ok(_) => panic!("unexpected success"),
        }

        match UnixListener::bind(socket_path) {
            Err(ref e) if e.kind() == io::ErrorKind::InvalidInput => {}
            Err(e) => panic!("unexpected error {}", e),
            Ok(_) => panic!("unexpected success"),
        }
    }
}
