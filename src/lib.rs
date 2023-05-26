use futures::ready;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::process::{Child, Command};

pub struct AsyncPty {
    master: AsyncFd<File>,
    _child: Child,
}

impl AsyncPty {
    pub fn open() -> io::Result<Self> {
        let mut cmd = Command::new("fish");
        let master = unsafe {
            let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let rc = libc::grantpt(fd);
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            let rc = libc::unlockpt(fd);
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut buf = [0 as libc::c_char; 1024];
            // for portability use ptsname
            let rc = libc::ptsname_r(fd, buf.as_mut_ptr(), buf.len());
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            let winsize = libc::winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let rc = libc::ioctl(fd, libc::TIOCSWINSZ, &winsize);
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }

            let slave_fd = libc::open(buf.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }

            cmd.pre_exec(move || {
                libc::close(fd);
                libc::setsid();

                let rc = libc::ioctl(slave_fd, libc::TIOCSCTTY);
                if rc == -1 {
                    return Err(io::Error::last_os_error());
                }

                libc::dup2(slave_fd, 0);
                libc::dup2(slave_fd, 1);
                libc::dup2(slave_fd, 2);
                libc::close(slave_fd);

                Ok(())
            });

            File::from_raw_fd(fd)
        };

        Ok(Self {
            master: AsyncFd::new(master)?,
            _child: cmd.spawn()?,
        })
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        unsafe {
            let winsize = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let rc = libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &winsize);
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub async fn read(&self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.master.readable().await?;

            match guard.try_io(|master| master.get_ref().read(out)) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }

    pub async fn write(&self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.master.writable().await?;

            match guard.try_io(|master| master.get_ref().write(buf)) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncRead for AsyncPty {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.master.poll_read_ready(cx))?;

            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|master| master.get_ref().read(unfilled)) {
                Ok(Ok(len)) => {
                    buf.advance(len);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for AsyncPty {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.master.poll_write_ready(cx))?;

            match guard.try_io(|master| master.get_ref().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn my_test() {
        let pty = AsyncPty::open().unwrap();
        let mut buf = [0u8; 512];
        pty.write(b"echo `date` > /tmp/term\n\r").await;
        pty.read(&mut buf).await;
        assert!(true);
    }
}
