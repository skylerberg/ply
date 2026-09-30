// The request path's floor: the answer `examples/desk.ply` gives `GET /health`, served with no
// language in the path, so that a row against it and a row against the desk differ only in the
// server. `benches/corpus.sh` builds it with `cc` and binds it as `http_floor`, and the corpus loads
// it with the client it loads the desk with.
//
//   floor sequential|threads [PORT]
//
// Listens on 127.0.0.1 at PORT (any free port when it is 0 or left off), prints `listening on N`,
// and answers every request on a connection until the client asks to close or goes away:
// `sequential` one connection at a time, `threads` a thread per connection. It runs until it is
// signalled.

#include <arpa/inet.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/socket.h>
#include <unistd.h>

#define BODY "{\"routes\":11,\"service\":\"parts desk\"}"

// The desk's own fields in `http::encode`'s order: the program's, the length, then `Connection:
// close` on the exchange that ends the connection. The corpus checks this against the desk's answer
// before it measures anything.
#define HEAD_OPEN "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nContent-Length: %zu\r\n"

// A head longer than this is not one the load client sends, so the connection is dropped.
#define MAX_HEAD 8192

static char kept[256];
static char closing[256];
static size_t kept_len;
static size_t closing_len;

static void die(const char *what) {
    perror(what);
    exit(1);
}

static void compose(void) {
    int n = snprintf(kept, sizeof kept, HEAD_OPEN "\r\n%s", strlen(BODY), BODY);
    int m = snprintf(closing, sizeof closing, HEAD_OPEN "Connection: close\r\n\r\n%s", strlen(BODY), BODY);
    if (n < 0 || m < 0 || (size_t)n >= sizeof kept || (size_t)m >= sizeof closing) {
        fprintf(stderr, "the response does not fit its buffer\n");
        exit(1);
    }
    kept_len = (size_t)n;
    closing_len = (size_t)m;
}

static int write_all(int fd, const char *bytes, size_t len) {
    while (len > 0) {
        ssize_t wrote = write(fd, bytes, len);
        if (wrote <= 0) {
            return -1;
        }
        bytes += wrote;
        len -= (size_t)wrote;
    }
    return 0;
}

// Where the head in `buf[0..len)` ends, just past its blank line, or 0 while it has not.
static size_t head_end(const char *buf, size_t len) {
    for (size_t i = 3; i < len; i++) {
        if (buf[i - 3] == '\r' && buf[i - 2] == '\n' && buf[i - 1] == '\r' && buf[i] == '\n') {
            return i + 1;
        }
    }
    return 0;
}

// Whether the head asks for the connection to close: a `Connection: close` field, in any case.
static int asks_to_close(const char *head, size_t len) {
    static const char field[] = "\r\nconnection: close";
    size_t want = sizeof field - 1;
    for (size_t i = 0; i + want <= len; i++) {
        if (strncasecmp(head + i, field, want) == 0) {
            return 1;
        }
    }
    return 0;
}

static void serve_connection(int fd) {
    int on = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &on, sizeof on);
    char buf[MAX_HEAD];
    size_t held = 0;
    for (;;) {
        size_t end = head_end(buf, held);
        if (end == 0) {
            if (held == sizeof buf) {
                break;
            }
            ssize_t got = read(fd, buf + held, sizeof buf - held);
            if (got <= 0) {
                break;
            }
            held += (size_t)got;
            continue;
        }
        // A `GET` carries no body, so the next request begins where this head ends.
        int close_after = asks_to_close(buf, end);
        if (close_after ? write_all(fd, closing, closing_len) : write_all(fd, kept, kept_len)) {
            break;
        }
        if (close_after) {
            break;
        }
        memmove(buf, buf + end, held - end);
        held -= end;
    }
    close(fd);
}

static void *connection_thread(void *raw) {
    int fd = (int)(long)raw;
    serve_connection(fd);
    return NULL;
}

int main(int argc, char **argv) {
    if (argc < 2 || argc > 3 || (strcmp(argv[1], "sequential") != 0 && strcmp(argv[1], "threads") != 0)) {
        fprintf(stderr, "usage: floor sequential|threads [PORT]\n");
        return 2;
    }
    int threads = strcmp(argv[1], "threads") == 0;
    long port = argc == 3 ? strtol(argv[2], NULL, 10) : 0;
    if (port < 0 || port > 65535) {
        fprintf(stderr, "a port is 0..65535\n");
        return 2;
    }
    // A client that hangs up mid-answer is a failed write, not the end of the floor.
    signal(SIGPIPE, SIG_IGN);
    compose();

    int listener = socket(AF_INET, SOCK_STREAM, 0);
    if (listener < 0) {
        die("socket");
    }
    int on = 1;
    setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on);
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof addr);
    addr.sin_family = AF_INET;
    addr.sin_port = htons((unsigned short)port);
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(listener, (struct sockaddr *)&addr, sizeof addr) != 0) {
        die("bind");
    }
    if (listen(listener, 1024) != 0) {
        die("listen");
    }
    socklen_t addr_len = sizeof addr;
    if (getsockname(listener, (struct sockaddr *)&addr, &addr_len) != 0) {
        die("getsockname");
    }
    printf("listening on %d\n", ntohs(addr.sin_port));
    fflush(stdout);

    for (;;) {
        int fd = accept(listener, NULL, NULL);
        if (fd < 0) {
            continue;
        }
        if (!threads) {
            serve_connection(fd);
            continue;
        }
        pthread_t thread;
        if (pthread_create(&thread, NULL, connection_thread, (void *)(long)fd) != 0) {
            close(fd);
            continue;
        }
        pthread_detach(thread);
    }
}
