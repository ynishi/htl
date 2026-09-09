/* A C caller for lib{{mod}}, and the shape a C host has to keep.
 *
 * What C gets wrong here by default: `char *{{mod}}_greet(...)` hands back memory this
 * library allocated, and C gives you no reason to think about it — printf takes the
 * pointer and the leak is invisible. Every one of them goes back to {{mod}}_free(), on
 * the failure path too, which is why the calls below hand the pointer to `show()`
 * instead of using it in place.
 *
 * Build and run (after `cargo build` in the project root):
 *
 *     make -C examples/c run
 *
 * or by hand, from the project root:
 *
 *     cc -I include -o /tmp/{{mod}}_c examples/c/main.c -L target/debug -l{{mod}} \
 *        -Wl,-rpath,target/debug && /tmp/{{mod}}_c
 */

#include <stdio.h>
#include <string.h>

#include "{{mod}}.h"

/* The message that goes with the last failure on this thread. The pointer belongs to the
 * library and is only good until the next call, so it is used and not kept. */
static const char *why(void) {
   const char *m = {{mod}}_last_error();
   return m ? m : "(no message)";
}

/* Print a `char *` the library returned, then give it back. NULL means the call failed,
 * and then the status says which kind of failure it was. */
static void show(const char *what, char *owned) {
   if (owned == NULL) {
      printf("%-14s -> NULL, status %d: %s\n", what, {{mod}}_last_status(), why());
      return;
   }
   printf("%-14s -> %s\n", what, owned);
   {{mod}}_free(owned);
}

int main(void) {
   printf("abi_version    -> %d (header says %d)\n", {{mod}}_abi_version(), {{MOD}}_ABI_VERSION);
   printf("version        -> %s\n", {{mod}}_version());
   printf("threadsafe     -> %d (0 = one handle per thread)\n", {{mod}}_threadsafe());

   /* Options are one JSON object: everything the library would otherwise have to read
    * from the environment or the working directory is passed in. */
   {{mod}}_handle *h = {{mod}}_open("{\"greeter\":\"the C host\"}");
   if (h == NULL) {
      fprintf(stderr, "open failed: %s\n", why());
      return 1;
   }

   show("greet", {{mod}}_greet(h, "Ada"));
   show("state", {{mod}}_state(h));

   /* An int is a status, never a value: the count comes back through the out-parameter. */
   int greeted = -1;
   if ({{mod}}_greeted(h, &greeted) == {{MOD}}_OK) {
      printf("%-14s -> %d\n", "greeted", greeted);
   }

   /* Error path 1: the failure is Lua's — the Teal module refuses an empty name — and it
    * crosses as NULL with the LUA status rather than as a crash or an empty string. */
   show("greet(\"\")", {{mod}}_greet(h, ""));

   /* Error path 2: the failure is the Rust side's, on a function whose return *is* the
    * status. Reset twice: the second one has nothing to do and says so. */
   printf("%-14s -> status %d\n", "reset", {{mod}}_reset(h));
   int again = {{mod}}_reset(h);
   printf("%-14s -> status %d: %s\n", "reset again", again, why());

   /* A message of your own, copied into your own buffer, for a host that cannot hold a
    * pointer the library owns. The return is what the message needs, so a bigger buffer
    * would hold more of it. */
   char buf[24];
   int needed = {{mod}}_last_error_into(buf, (int) sizeof buf);
   printf("%-14s -> needed %d, got \"%s\"\n", "into(24)", needed, buf);

   {{mod}}_close(h);
   printf("closed\n");
   return 0;
}
