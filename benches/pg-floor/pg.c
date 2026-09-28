// The three things the corpus's database benches need from libpq rather than Ply: a floor with no Ply
// in the path, and a session that holds a row lock while other work happens.
//
//   pg floor <url> <workload> <connections> <operations> <base>
//
// Runs `operations` statements on each of `connections` connections at once, each statement prepared
// once per connection, and prints one line: `floor <microseconds>`. The clock runs around the
// statements — not the connect, but the prepare, which is what the rung it is compared with does —
// so the number means the same thing the Ply rung's does.
//
//   pg lock <url> <sql>
//
// Begins, runs `sql` (a `select ... for update`), prints `locked`, and waits for one line on stdin,
// then rolls back and prints `released`. Closing stdin releases it too.
//
// A workload is one of the Ply rung's four labels: `select`, `select $1`, `insert`, `transaction`.
// Statement text, parameter binding and the row shapes are the Ply program's: this is the same work
// with the language taken out, which is the only reason it is here at all.

#include <libpq-fe.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define SELECT_ALL "select sku, name, price, n from part order by sku limit 1"
#define SELECT_BY "select sku, name, price, n from part where sku = $1"
#define INSERT_ONE "insert into part (sku, name, price, n) values ($1, $2, $3, $4)"

// The price every Ply rung binds, as the numeric text a `db::PNumeric(1.2500m)` becomes on the wire.
#define PRICE "1.2500"

// The keys the fixture holds, so a keyed select hits a row.
#define KEYS 64

static void die(const char *what, PGconn *conn) {
    if (conn != NULL) {
        fprintf(stderr, "%s: %s", what, PQerrorMessage(conn));
    } else {
        fprintf(stderr, "%s", what);
    }
    exit(1);
}

static PGconn *connect_to(const char *url) {
    PGconn *conn = PQconnectdb(url);
    if (PQstatus(conn) != CONNECTION_OK) {
        die("connecting", conn);
    }
    return conn;
}

static void exec_ok(PGconn *conn, const char *sql) {
    PGresult *result = PQexec(conn, sql);
    ExecStatusType status = PQresultStatus(result);
    if (status != PGRES_COMMAND_OK && status != PGRES_TUPLES_OK) {
        fprintf(stderr, "%s: %s", sql, PQerrorMessage(conn));
        PQclear(result);
        exit(1);
    }
    PQclear(result);
}

static void prepare(PGconn *conn, const char *name, const char *sql) {
    // No parameter types: every parameter here is a column's own type, which the server infers.
    PGresult *result = PQprepare(conn, name, sql, 0, NULL);
    if (PQresultStatus(result) != PGRES_COMMAND_OK) {
        fprintf(stderr, "preparing %s: %s", name, PQerrorMessage(conn));
        PQclear(result);
        exit(1);
    }
    PQclear(result);
}

static void run_prepared(PGconn *conn, const char *name, int argc, const char *const *argv) {
    PGresult *result = PQexecPrepared(conn, name, argc, argv, NULL, NULL, 0);
    ExecStatusType status = PQresultStatus(result);
    if (status != PGRES_COMMAND_OK && status != PGRES_TUPLES_OK) {
        fprintf(stderr, "%s: %s", name, PQerrorMessage(conn));
        PQclear(result);
        exit(1);
    }
    // A `select`'s rows are read here rather than left to the server: the floor is the whole round
    // trip, and a rung that walked the rows would be measuring something else.
    if (status == PGRES_TUPLES_OK && PQntuples(result) > 0 && PQnfields(result) > 0) {
        (void)PQgetvalue(result, 0, 0);
    }
    PQclear(result);
}

struct work {
    const char *url;
    int workload;
    unsigned per;
    long base;
    unsigned slot;
};

enum { W_SELECT, W_SELECT_PARAM, W_INSERT, W_TRANSACTION };

static void *drive(void *raw) {
    const struct work *w = raw;
    PGconn *conn = connect_to(w->url);
    prepare(conn, "select_all", SELECT_ALL);
    prepare(conn, "select_by", SELECT_BY);
    prepare(conn, "insert_one", INSERT_ONE);
    char sku[64];
    char n[32];
    for (unsigned i = 0; i < w->per; i++) {
        switch (w->workload) {
            case W_SELECT:
                run_prepared(conn, "select_all", 0, NULL);
                break;
            case W_SELECT_PARAM:
                snprintf(sku, sizeof sku, "sku-%u", i % KEYS);
                run_prepared(conn, "select_by", 1, (const char *const[]){sku});
                break;
            case W_INSERT: {
                long id = w->base + (long)i;
                snprintf(sku, sizeof sku, "sku-%ld", id);
                snprintf(n, sizeof n, "%ld", id);
                run_prepared(conn, "insert_one", 4, (const char *const[]){sku, "a part", PRICE, n});
                break;
            }
            case W_TRANSACTION: {
                long id = w->base + (long)i;
                snprintf(sku, sizeof sku, "sku-%ld", id);
                snprintf(n, sizeof n, "%ld", id);
                exec_ok(conn, "begin");
                run_prepared(conn, "insert_one", 4, (const char *const[]){sku, "a part", PRICE, n});
                exec_ok(conn, "commit");
                break;
            }
        }
    }
    PQfinish(conn);
    return NULL;
}

static long micros_between(const struct timespec *from, const struct timespec *to) {
    return (to->tv_sec - from->tv_sec) * 1000000L + (to->tv_nsec - from->tv_nsec) / 1000L;
}

static int floor_run(const char *url, const char *workload, unsigned connections, unsigned per, long base) {
    int kind = -1;
    if (strcmp(workload, "select") == 0) {
        kind = W_SELECT;
    } else if (strcmp(workload, "select $1") == 0) {
        kind = W_SELECT_PARAM;
    } else if (strcmp(workload, "insert") == 0) {
        kind = W_INSERT;
    } else if (strcmp(workload, "transaction") == 0) {
        kind = W_TRANSACTION;
    } else {
        fprintf(stderr, "unknown workload `%s`\n", workload);
        return 1;
    }

    struct work *works = calloc(connections, sizeof *works);
    pthread_t *threads = calloc(connections, sizeof *threads);
    if (works == NULL || threads == NULL) {
        die("allocating the floor's threads", NULL);
    }
    for (unsigned slot = 0; slot < connections; slot++) {
        works[slot] = (struct work){
            .url = url,
            .workload = kind,
            .per = per,
            .base = base + (long)slot * (long)per,
            .slot = slot,
        };
    }

    struct timespec from, to;
    clock_gettime(CLOCK_MONOTONIC, &from);
    for (unsigned slot = 0; slot < connections; slot++) {
        if (pthread_create(&threads[slot], NULL, drive, &works[slot]) != 0) {
            die("starting a floor connection", NULL);
        }
    }
    for (unsigned slot = 0; slot < connections; slot++) {
        pthread_join(threads[slot], NULL);
    }
    clock_gettime(CLOCK_MONOTONIC, &to);

    printf("floor %ld\n", micros_between(&from, &to));
    fflush(stdout);
    free(threads);
    free(works);
    return 0;
}

static int lock_hold(const char *url, const char *sql) {
    PGconn *conn = connect_to(url);
    exec_ok(conn, "begin");
    // A `select ... for update` takes the row lock, which is held until the rollback below.
    ExecStatusType locked = PQresultStatus(PQexec(conn, sql));
    if (locked != PGRES_TUPLES_OK) {
        die(sql, conn);
    }
    printf("locked\n");
    fflush(stdout);
    // Whatever the harness writes releases it; so does closing the pipe, which is how a test that
    // has gone wrong still lets go.
    char line[2];
    (void)fgets(line, sizeof line, stdin);
    exec_ok(conn, "rollback");
    printf("released\n");
    fflush(stdout);
    PQfinish(conn);
    return 0;
}

// The fixture's DDL, which the language's own client refuses on purpose: it runs `drop`, `create`
// and `truncate`, and a program that can run those is a program that can lose a database.
static int sql_run(const char *url, const char *sql) {
    PGconn *conn = connect_to(url);
    exec_ok(conn, sql);
    PQfinish(conn);
    return 0;
}

int main(int argc, char **argv) {
    if (argc == 7 && strcmp(argv[1], "floor") == 0) {
        return floor_run(argv[2], argv[3], (unsigned)strtoul(argv[4], NULL, 10),
                         (unsigned)strtoul(argv[5], NULL, 10), strtol(argv[6], NULL, 10));
    }
    if (argc == 4 && strcmp(argv[1], "lock") == 0) {
        return lock_hold(argv[2], argv[3]);
    }
    if (argc == 4 && strcmp(argv[1], "sql") == 0) {
        return sql_run(argv[2], argv[3]);
    }
    fprintf(stderr,
            "usage: pg floor <url> <workload> <connections> <operations> <base>\n"
            "       pg lock <url> <sql>\n"
            "       pg sql <url> <statements>\n");
    return 1;
}
