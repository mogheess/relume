/* Test helper: apply mkdir/put/del operations to an NTFS image via libntfs-3g.
   Usage: ntfsops IMAGE < ops   (ops lines: "mkdir /a/b", "put LOCAL /path", "del /path") */
#include "config.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include "types.h"
#include "volume.h"
#include "inode.h"
#include "dir.h"
#include "attrib.h"
#include "unistr.h"

static ntfs_volume *vol;

static int split(const char *path, char *parent, char *name) {
    const char *s = strrchr(path, '/');
    if (!s) return -1;
    if (s == path) strcpy(parent, "/"); else { memcpy(parent, path, s - path); parent[s - path] = 0; }
    strcpy(name, s + 1);
    return 0;
}

static ntfs_inode *create(const char *path, mode_t type) {
    char parent[1024], name[256];
    split(path, parent, name);
    ntfs_inode *dir = ntfs_pathname_to_inode(vol, NULL, parent);
    if (!dir) { perror(parent); exit(1); }
    ntfschar *u = NULL; int len = ntfs_mbstoucs(name, &u);
    ntfs_inode *ni = ntfs_create(dir, const_cpu_to_le32(0), u, len, type);
    if (!ni) { perror(path); exit(1); }
    ntfs_inode_close(dir);
    free(u);
    return ni;
}

int main(int argc, char **argv) {
    vol = ntfs_mount(argv[1], 0);
    if (!vol) { perror("mount"); return 1; }
    char line[2048];
    while (fgets(line, sizeof line, stdin)) {
        line[strcspn(line, "\n")] = 0;
        char a[1024], b[1024];
        if (sscanf(line, "mkdir %1023s", a) == 1) {
            ntfs_inode_close(create(a, S_IFDIR));
        } else if (sscanf(line, "put %1023s %1023s", a, b) == 2) {
            FILE *f = fopen(a, "rb"); if (!f) { perror(a); return 1; }
            ntfs_inode *ni = create(b, S_IFREG);
            ntfs_attr *na = ntfs_attr_open(ni, AT_DATA, AT_UNNAMED, 0);
            static char buf[1 << 20]; size_t n; s64 pos = 0;
            while ((n = fread(buf, 1, sizeof buf, f)) > 0) {
                if (ntfs_attr_pwrite(na, pos, n, buf) != (s64)n) { perror("write"); return 1; }
                pos += n;
            }
            fclose(f); ntfs_attr_close(na); ntfs_inode_close(ni);
        } else if (sscanf(line, "del %1023s", a) == 1) {
            char parent[1024], name[256];
            split(a, parent, name);
            ntfs_inode *ni = ntfs_pathname_to_inode(vol, NULL, a);
            ntfs_inode *dir = ntfs_pathname_to_inode(vol, NULL, parent);
            if (!ni || !dir) { perror(a); return 1; }
            ntfschar *u = NULL; int len = ntfs_mbstoucs(name, &u);
            if (ntfs_delete(vol, a, ni, dir, u, len)) { perror("delete"); return 1; }
            free(u);
        }
    }
    ntfs_umount(vol, FALSE);
    return 0;
}
