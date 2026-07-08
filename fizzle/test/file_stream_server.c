#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static const char expected[] =
    "file-backed payload over fizzle\n"
    "second recv-sized segment with more bytes\n"
    "third segment keeps the stream open long enough\n"
    "final segment proves repeated recv aggregation\n";

static void die(const char *message) {
    perror(message);
    exit(1);
}

int main(void) {
    int listen_fd = socket(AF_INET, SOCK_STREAM, 0);
    if (listen_fd < 0) {
        die("socket");
    }

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(39175);
    if (inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr) != 1) {
        die("inet_pton");
    }

    if (bind(listen_fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        die("bind");
    }

    if (listen(listen_fd, 1) < 0) {
        die("listen");
    }

    int client_fd = accept(listen_fd, NULL, NULL);
    if (client_fd < 0) {
        die("accept");
    }

    char buf[sizeof(expected)];
    memset(buf, 0, sizeof(buf));

    size_t total = 0;
    size_t recv_calls = 0;
    while (total < sizeof(expected) - 1) {
        size_t remaining = sizeof(expected) - 1 - total;
        size_t request = remaining < 7 ? remaining : 7;
        ssize_t got = recv(client_fd, buf + total, request, 0);
        if (got < 0) {
            die("recv");
        }
        if (got == 0) {
            fprintf(stderr, "recv returned EOF after %zu bytes\n", total);
            return 1;
        }
        total += (size_t)got;
        recv_calls++;
    }

    if (total != sizeof(expected) - 1 || memcmp(buf, expected, sizeof(expected) - 1) != 0) {
        fprintf(stderr, "unexpected payload: got %zu bytes: %.*s\n", total, (int)total, buf);
        return 1;
    }
    if (recv_calls < 2) {
        fprintf(stderr, "expected multiple recv calls, got %zu\n", recv_calls);
        return 1;
    }

    close(client_fd);
    close(listen_fd);
    return 0;
}
