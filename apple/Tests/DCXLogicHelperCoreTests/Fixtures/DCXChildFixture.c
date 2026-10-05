/* Test-only fixed scenarios. No device, serial, socket, or audio operations.
 * dec-native-studio-20261004 / R-N13. This executable is never bundled in the
 * product; the admitted native test runner compiles it inside TEST_TMPDIR. */
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static int write_all(int fd, const void *bytes, size_t count) {
    const unsigned char *cursor = bytes;
    while (count) {
        ssize_t written = write(fd, cursor, count);
        if (written < 0 && errno == EINTR) continue;
        if (written <= 0) return -1;
        cursor += written;
        count -= (size_t)written;
    }
    return 0;
}

static void pause_milliseconds(long milliseconds) {
    struct timespec delay = { milliseconds / 1000, (milliseconds % 1000) * 1000000 };
    while (nanosleep(&delay, &delay) != 0 && errno == EINTR) {}
}

static int marker(const char *path) {
    int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0600);
    if (fd < 0) return -1;
    int result = write_all(fd, "fixture\n", 8);
    if (close(fd) != 0) result = -1;
    return result;
}

static int output(int fd, unsigned char value, size_t count) {
    unsigned char chunk[16384];
    memset(chunk, value, sizeof chunk);
    while (count) {
        size_t next = count < sizeof chunk ? count : sizeof chunk;
        if (write_all(fd, chunk, next) != 0) return -1;
        count -= next;
    }
    return 0;
}

static int hold_stdin(const char *ready, const char *release, const char *done) {
    struct stat metadata;
    if (fstat(STDIN_FILENO, &metadata) != 0 || !S_ISREG(metadata.st_mode)) return 74;
    if (marker(ready) != 0) return 74;
    /* Always exits voluntarily even if the XCTest process fails. */
    int requested = 0;
    for (unsigned i = 0; i != 1000; ++i) {
        if (access(release, F_OK) == 0) { requested = 1; break; }
        pause_milliseconds(10);
    }
    if (close(STDIN_FILENO) != 0 || marker(done) != 0) return 74;
    return requested ? 0 : 75;
}

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "dual-pipes") == 0) {
        for (unsigned i = 0; i != 16; ++i) {
            if (output(STDOUT_FILENO, 'O', 16384) != 0 ||
                output(STDERR_FILENO, 'E', 16384) != 0) return 74;
        }
        return 23;
    }
    if (argc == 2 && strcmp(argv[1], "exact-limit") == 0)
        return output(STDOUT_FILENO, 'L', 1048576) == 0 ? 0 : 74;
    if (argc == 2 && strcmp(argv[1], "overflow-stdout") == 0)
        return output(STDOUT_FILENO, 'X', 1048577) == 0 ? 0 : 74;
    if (argc == 2 && strcmp(argv[1], "overflow-stderr") == 0)
        return output(STDERR_FILENO, 'X', 1048577) == 0 ? 0 : 74;
    if (argc == 2 && strcmp(argv[1], "deadline") == 0) {
        /* Exercise existing runner escalation only on this owned fixture. */
        signal(SIGTERM, SIG_IGN);
        pause_milliseconds(10000);
        return 0;
    }
    if (argc == 3 && strcmp(argv[1], "retained-pipes") == 0) {
        pid_t child = fork();
        if (child < 0) return 74;
        if (child == 0) {
            close(STDIN_FILENO);
            /* Keep both output descriptors open past the runner's deadline. */
            pause_milliseconds(4000);
            _exit(marker(argv[2]) == 0 ? 0 : 74);
        }
        return 0;
    }
    if (argc == 5 && strcmp(argv[1], "hold-stdin") == 0)
        return hold_stdin(argv[2], argv[3], argv[4]);
    if (argc == 6 && strcmp(argv[1], "voluntary-owner-exit") == 0) {
        /* Mirrors the helper's open-description flock and stdin handoff; the
         * Swift suite separately exercises childStandardInput() itself. */
        int fd = open(argv[2], O_RDWR | O_NOFOLLOW);
        struct stat metadata;
        if (fd < 0 || fstat(fd, &metadata) != 0 || !S_ISREG(metadata.st_mode) ||
            metadata.st_uid != geteuid() || (metadata.st_mode & 077) != 0 ||
            flock(fd, LOCK_EX | LOCK_NB) != 0 || dup2(fd, STDIN_FILENO) < 0) return 74;
        close(fd);
        pid_t child = fork();
        if (child < 0) return 74;
        if (child == 0) {
            close(STDOUT_FILENO);
            close(STDERR_FILENO);
            _exit(hold_stdin(argv[3], argv[4], argv[5]));
        }
        /* Exit without an explicit LOCK_UN. Inherited stdin is the sole
         * surviving open-file description once this fixture parent exits. */
        return 0;
    }
    return 64;
}
