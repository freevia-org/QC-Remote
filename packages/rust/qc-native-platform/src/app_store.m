#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <ServiceManagement/ServiceManagement.h>
#include <stdlib.h>
#include <pwd.h>
#include <unistd.h>
#include <string.h>

// The macOS team-prefixed group is validated against the signing team.
// Resolve through Foundation rather than constructing a protected container path.
char *qc_store_shared_directory(void) {
    @autoreleasepool {
        NSURL *url = [[NSFileManager defaultManager]
            containerURLForSecurityApplicationGroupIdentifier:@"8C782Q826S.org.freevia.qcremote"];
        if (!url) return NULL;
        return strdup([[url URLByAppendingPathComponent:@"state" isDirectory:YES] fileSystemRepresentation]);
    }
}

// A short-lived login launcher opens the independent menu app and exits zero.
// Unregistering this service therefore cannot terminate the resident broker.
char *qc_store_set_login_enabled(bool enabled) {
    @autoreleasepool {
        SMAppService *service = [SMAppService loginItemServiceWithIdentifier:@"org.freevia.qcremote.login"];
        SMAppServiceStatus status = service.status;
        if ((enabled && status == SMAppServiceStatusEnabled) ||
            (!enabled && (status == SMAppServiceStatusNotRegistered || status == SMAppServiceStatusNotFound))) return NULL;
        NSError *error = nil;
        BOOL success = enabled ? [service registerAndReturnError:&error] : [service unregisterAndReturnError:&error];
        if (!success) return strdup((error.localizedDescription ?: @"Could not update QC Remote login item").UTF8String);
        if (enabled && service.status == SMAppServiceStatusRequiresApproval)
            return strdup("Approve QC Remote in System Settings > General > Login Items before enabling background startup.");
        return NULL;
    }
}

// Bookmarks grant only the specific host configuration folder selected by the
// owner. No home-directory entitlement or sandbox exception is used.
static NSMutableDictionary<NSString *, NSURL *> *activeScopes;
static NSString *const bookmarkKey = @"QCRemoteLocalClientFolderBookmarksV1";
static void restoreScopes(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        activeScopes = [NSMutableDictionary dictionary];
        NSDictionary *bookmarks = [[NSUserDefaults standardUserDefaults] dictionaryForKey:bookmarkKey];
        for (NSString *path in bookmarks) {
            NSData *data = bookmarks[path];
            if (![data isKindOfClass:[NSData class]]) continue;
            BOOL stale = NO;
            NSURL *url = [NSURL URLByResolvingBookmarkData:data
                options:NSURLBookmarkResolutionWithSecurityScope | NSURLBookmarkResolutionWithoutUI
                relativeToURL:nil bookmarkDataIsStale:&stale error:nil];
            if (url && [[url.path stringByStandardizingPath] isEqualToString:[path stringByStandardizingPath]] && [url startAccessingSecurityScopedResource]) activeScopes[path] = url;
        }
    });
}
char *qc_store_user_home(void) {
    @autoreleasepool {
        restoreScopes();
        // Foundation's home APIs are rewritten to the app container in the
        // sandbox. The user record supplies the real suggested location only;
        // filesystem access still requires the owner's Powerbox selection.
        struct passwd record;
        struct passwd *result = NULL;
        char buffer[16384];
        if (getpwuid_r(getuid(), &record, buffer, sizeof(buffer), &result) != 0 ||
            !result || !record.pw_dir || record.pw_dir[0] != '/') return NULL;
        return strdup(record.pw_dir);
    }
}
char *qc_store_authorize_folder(const char *filesystemPath) {
    @autoreleasepool {
        restoreScopes();
        NSString *path = [[NSFileManager defaultManager] stringWithFileSystemRepresentation:filesystemPath length:strlen(filesystemPath)];
        if (!path.isAbsolutePath) return strdup("The client folder path is not absolute.");
        @synchronized(activeScopes) { if (activeScopes[path]) return NULL; }
        __block NSString *failure = nil;
        void (^selectFolder)(void) = ^{
            NSOpenPanel *panel = [NSOpenPanel openPanel];
            panel.title = @"Allow LOCAL AI pairing";
            panel.message = [NSString stringWithFormat:@"Select %@ to let QC Remote manage its MCP entry and device skill. Other configuration entries are preserved.",path];
            panel.prompt = @"Allow pairing";
            panel.canChooseFiles = NO;
            panel.canChooseDirectories = YES;
            panel.allowsMultipleSelection = NO;
            panel.canCreateDirectories = YES;
            panel.showsHiddenFiles = YES;
            panel.directoryURL = [NSURL fileURLWithPath:path isDirectory:YES];
            if ([panel runModal] != NSModalResponseOK) { failure = @"LOCAL pairing folder access was cancelled."; return; }
            NSURL *url = panel.URL;
            if (![[url.path stringByStandardizingPath] isEqualToString:[path stringByStandardizingPath]]) {
                failure = @"Select the requested AI client's configuration folder to pair it."; return;
            }
            if (![url startAccessingSecurityScopedResource]) { failure = @"Could not access the selected client folder."; return; }
            NSError *error = nil;
            NSData *bookmark = [url bookmarkDataWithOptions:NSURLBookmarkCreationWithSecurityScope includingResourceValuesForKeys:nil relativeToURL:nil error:&error];
            if (!bookmark) { [url stopAccessingSecurityScopedResource]; failure = error.localizedDescription ?: @"Could not save client folder access."; return; }
            @synchronized(activeScopes) {
                NSMutableDictionary *bookmarks = [[[NSUserDefaults standardUserDefaults] dictionaryForKey:bookmarkKey] mutableCopy] ?: [NSMutableDictionary dictionary];
                bookmarks[path] = bookmark;
                [[NSUserDefaults standardUserDefaults] setObject:bookmarks forKey:bookmarkKey];
                activeScopes[path] = url;
            }
        };
        if ([NSThread isMainThread]) selectFolder(); else dispatch_sync(dispatch_get_main_queue(), selectFolder);
        return failure ? strdup(failure.UTF8String) : NULL;
    }
}

// Only the owning process may inspect these FDs under App Sandbox. The private
// Unix witness answers for itself; it never inspects another process or exposes
// socket inventory. The exact IPv4 loopback tuple prevents proof relaying.
#include <libproc.h>
#include <sys/proc_info.h>
#include <arpa/inet.h>
#include <errno.h>
#include <time.h>
int qc_store_owns_tcp_pair(uint16_t server_port, uint16_t client_port) {
    if (!server_port || !client_port) return -EINVAL;
    int capacity = proc_pidinfo(getpid(), PROC_PIDLISTFDS, 0, NULL, 0);
    if (capacity <= 0) return -(errno ?: EIO);
    if (capacity > 16384 * sizeof(struct proc_fdinfo)) return -EMFILE;
    struct proc_fdinfo *fds = malloc(capacity);
    if (!fds) return -ENOMEM;
    int bytes = proc_pidinfo(getpid(), PROC_PIDLISTFDS, 0, fds, capacity);
    if (bytes <= 0 || bytes > capacity || bytes % sizeof(*fds)) { int error = errno ?: EIO; free(fds); return -error; }
    struct timespec start, now;
    clock_gettime(CLOCK_MONOTONIC, &start);
    int result = 0;
    for (int i = 0; i < bytes / sizeof(*fds); i++) {
        if ((i & 15) == 0) {
            clock_gettime(CLOCK_MONOTONIC, &now);
            if ((now.tv_sec - start.tv_sec) * 1000000000LL + now.tv_nsec - start.tv_nsec > 100000000LL) { result = -ETIMEDOUT; break; }
        }
        if (fds[i].proc_fdtype != PROX_FDTYPE_SOCKET) continue;
        struct socket_fdinfo socket;
        int size = proc_pidfdinfo(getpid(), fds[i].proc_fd, PROC_PIDFDSOCKETINFO, &socket, sizeof(socket));
        if (size != sizeof(socket)) {
            if (size <= 0 && errno == EBADF) continue; // a concurrently closed FD
            result = -(errno ?: EIO); break;
        }
        if (socket.psi.soi_family != AF_INET || socket.psi.soi_kind != SOCKINFO_TCP) continue;
        struct tcp_sockinfo *tcp = &socket.psi.soi_proto.pri_tcp;
        struct in_sockinfo *ip = &tcp->tcpsi_ini;
        if (tcp->tcpsi_state == TSI_S_ESTABLISHED &&
            ntohs((uint16_t)ip->insi_lport) == server_port &&
            ntohs((uint16_t)ip->insi_fport) == client_port &&
            ntohl(ip->insi_laddr.ina_46.i46a_addr4.s_addr) == INADDR_LOOPBACK &&
            ntohl(ip->insi_faddr.ina_46.i46a_addr4.s_addr) == INADDR_LOOPBACK) { result = 1; break; }
    }
    free(fds);
    return result;
}

bool qc_store_has_folder_access(const char *filesystemPath) {
    @autoreleasepool {
        restoreScopes();
        NSString *path = [[[NSFileManager defaultManager] stringWithFileSystemRepresentation:filesystemPath length:strlen(filesystemPath)] stringByStandardizingPath];
        @synchronized(activeScopes) {
            for (NSString *root in activeScopes) {
                if ([path isEqualToString:root] || [path hasPrefix:[root stringByAppendingString:@"/"]]) return true;
            }
        }
        return false;
    }
}
