#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>

int main() {
    char buf[20];

    FILE* file = fmemopen(buf, 20, "r+");
    assert(file != NULL);

    size_t written = fwrite("123456789012345678901234", 1, 24, file);
    assert(written == 24);
    printf("fwrite() -> %lu\n", written);

    int res = fflush(file);
    assert(res == EOF);
    
    fclose(file);
    return 0;
}