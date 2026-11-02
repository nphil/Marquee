/* Minimal in-process Wayland compositor for subtitle_wayland_smoke.rs. */
#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <wayland-client.h>
#include <wayland-server.h>

static const struct wl_interface region_iface;
static const struct wl_interface *get_region_types[] = { &region_iface, &wl_surface_interface };
static const struct wl_message manager_methods[] = {
    { "destroy", "", NULL }, { "get_region", "no", get_region_types }
};
static const struct wl_interface manager_iface = {
    "lumatv_subtitle_manager_v1", 1, 2, manager_methods, 0, NULL
};
static const struct wl_message region_methods[] = {
    { "destroy", "", NULL }, { "set_bounds", "iiiiii", NULL }
};
static const struct wl_interface region_iface = {
    "lumatv_subtitle_region_v1", 1, 2, region_methods, 0, NULL
};

static atomic_int bounds[6], updates, region_created, region_destroyed, manager_destroyed, commits;
static struct wl_display *server;
static pthread_t thread;
static struct wl_compositor *compositor;
static struct wl_surface *surface;
static struct wl_registry *registry;

static void region_destroy(struct wl_client *client, struct wl_resource *resource) {
    (void)client; atomic_fetch_add(&region_destroyed, 1); wl_resource_destroy(resource);
}
static void region_bounds(struct wl_client *client, struct wl_resource *resource,
                          int32_t cw, int32_t ch, int32_t x, int32_t y, int32_t w, int32_t h) {
    (void)client; (void)resource;
    int values[] = {cw, ch, x, y, w, h};
    for (int i = 0; i < 6; i++) atomic_store(&bounds[i], values[i]);
    atomic_fetch_add(&updates, 1);
}
static const struct { void (*destroy)(struct wl_client *, struct wl_resource *); void (*set_bounds)(struct wl_client *, struct wl_resource *, int32_t, int32_t, int32_t, int32_t, int32_t, int32_t); } region_impl = {region_destroy, region_bounds};
static void manager_destroy(struct wl_client *client, struct wl_resource *resource) {
    (void)client; atomic_fetch_add(&manager_destroyed, 1); wl_resource_destroy(resource);
}
static void get_region(struct wl_client *client, struct wl_resource *manager, uint32_t id, struct wl_resource *surface_resource) {
    (void)manager; assert(surface_resource);
    struct wl_resource *region = wl_resource_create(client, &region_iface, 1, id);
    assert(region); wl_resource_set_implementation(region, &region_impl, NULL, NULL);
    atomic_fetch_add(&region_created, 1);
}
static const struct { void (*destroy)(struct wl_client *, struct wl_resource *); void (*get_region)(struct wl_client *, struct wl_resource *, uint32_t, struct wl_resource *); } manager_impl = {manager_destroy, get_region};
static void bind_manager(struct wl_client *client, void *data, uint32_t version, uint32_t id) {
    (void)data; (void)version;
    struct wl_resource *resource = wl_resource_create(client, &manager_iface, 1, id);
    assert(resource); wl_resource_set_implementation(resource, &manager_impl, NULL, NULL);
}
static void surface_commit(struct wl_client *client, struct wl_resource *resource) {
    (void)client; (void)resource; atomic_fetch_add(&commits, 1);
}
static const struct wl_surface_interface surface_impl = { .commit = surface_commit };
static void create_surface(struct wl_client *client, struct wl_resource *compositor_resource, uint32_t id) {
    (void)compositor_resource;
    struct wl_resource *resource = wl_resource_create(client, &wl_surface_interface, 1, id);
    assert(resource); wl_resource_set_implementation(resource, &surface_impl, NULL, NULL);
}
static const struct wl_compositor_interface compositor_impl = { .create_surface = create_surface };
static void bind_compositor(struct wl_client *client, void *data, uint32_t version, uint32_t id) {
    (void)data; (void)version;
    struct wl_resource *resource = wl_resource_create(client, &wl_compositor_interface, 1, id);
    assert(resource); wl_resource_set_implementation(resource, &compositor_impl, NULL, NULL);
}
static void global(void *data, struct wl_registry *r, uint32_t name, const char *interface, uint32_t version) {
    (void)data; (void)version;
    if (__builtin_strcmp(interface, "wl_compositor") == 0)
        compositor = wl_registry_bind(r, name, &wl_compositor_interface, 1);
}
static void global_remove(void *data, struct wl_registry *r, uint32_t name) { (void)data; (void)r; (void)name; }
static const struct wl_registry_listener listener = {global, global_remove};
static void *run(void *unused) { (void)unused; wl_display_run(server); return NULL; }

void *smoke_start(int advertise) {
    atomic_store(&updates, 0); atomic_store(&region_created, 0); atomic_store(&region_destroyed, 0);
    atomic_store(&manager_destroyed, 0); atomic_store(&commits, 0);
    server = wl_display_create(); assert(server);
    assert(wl_global_create(server, &wl_compositor_interface, 1, NULL, bind_compositor));
    if (advertise) assert(wl_global_create(server, &manager_iface, 1, NULL, bind_manager));
    int sockets[2]; assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
    assert(wl_client_create(server, sockets[0]));
    assert(pthread_create(&thread, NULL, run, NULL) == 0);
    struct wl_display *display = wl_display_connect_to_fd(sockets[1]); assert(display);
    registry = wl_display_get_registry(display); assert(registry);
    assert(wl_registry_add_listener(registry, &listener, NULL) == 0);
    assert(wl_display_roundtrip(display) >= 0); assert(compositor);
    surface = wl_compositor_create_surface(compositor); assert(surface);
    return display;
}
void *smoke_surface(void) { return surface; }
void smoke_sync(void *display) { assert(wl_display_roundtrip(display) >= 0); }
int smoke_count(int which) {
    switch (which) { case 0: return atomic_load(&updates); case 1: return atomic_load(&region_created);
    case 2: return atomic_load(&region_destroyed); case 3: return atomic_load(&manager_destroyed);
    case 4: return atomic_load(&commits); default: return atomic_load(&bounds[which - 5]); }
}
void smoke_stop(void *display) {
    wl_proxy_destroy((struct wl_proxy *)surface); wl_compositor_destroy(compositor); wl_registry_destroy(registry);
    wl_display_disconnect(display); wl_display_terminate(server); pthread_join(thread, NULL); wl_display_destroy(server);
    compositor = NULL; surface = NULL; registry = NULL; server = NULL;
}
