# The C caller. `cargo build` first: this links against what it leaves in target/debug.
#
#     make -C examples/c run          # debug
#     make -C examples/c run PROFILE=release
#
# The library is loaded at run time, so the path it was linked from is baked in with
# -rpath; without that the loader would only look in the system directories.

PROFILE ?= debug
ROOT    := ../..
# Override when cargo was pointed elsewhere: make run LIBDIR=/path/to/target/debug
LIBDIR  ?= $(ROOT)/target/$(PROFILE)
CFLAGS  += -std=c11 -Wall -Wextra -I $(ROOT)/include

{{mod}}_c: main.c $(ROOT)/include/{{mod}}.h
	$(CC) $(CFLAGS) -o $@ main.c -L $(LIBDIR) -l{{mod}} -Wl,-rpath,$(abspath $(LIBDIR))

run: {{mod}}_c
	./{{mod}}_c

clean:
	rm -f {{mod}}_c

.PHONY: run clean
