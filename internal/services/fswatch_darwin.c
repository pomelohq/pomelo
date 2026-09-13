#include "fswatch_darwin.h"

extern void pomFSEventsCallback(int handle, const char *path);

static void pom_fsevents_cb(ConstFSEventStreamRef stream, void *info, size_t count,
                            void *paths, const FSEventStreamEventFlags flags[],
                            const FSEventStreamEventId ids[]) {
	char **list = (char **)paths;
	for (size_t i = 0; i < count; i++) {
		pomFSEventsCallback((int)(intptr_t)info, list[i]);
	}
}

FSEventStreamRef pom_fsevents_start(const char *path, int handle, dispatch_queue_t queue) {
	CFStringRef cfPath = CFStringCreateWithCString(NULL, path, kCFStringEncodingUTF8);
	if (cfPath == NULL) return NULL;
	CFArrayRef paths = CFArrayCreate(NULL, (const void **)&cfPath, 1, &kCFTypeArrayCallBacks);
	CFRelease(cfPath);
	if (paths == NULL) return NULL;

	FSEventStreamContext ctx = {0, (void *)(intptr_t)handle, NULL, NULL, NULL};
	FSEventStreamRef stream = FSEventStreamCreate(NULL, pom_fsevents_cb, &ctx, paths,
	                                             kFSEventStreamEventIdSinceNow, 0.3,
	                                             kFSEventStreamCreateFlagNoDefer |
	                                             kFSEventStreamCreateFlagFileEvents |
	                                             kFSEventStreamCreateFlagWatchRoot);
	CFRelease(paths);
	if (stream == NULL) return NULL;

	FSEventStreamSetDispatchQueue(stream, queue);
	if (!FSEventStreamStart(stream)) {
		FSEventStreamInvalidate(stream);
		FSEventStreamRelease(stream);
		return NULL;
	}
	return stream;
}

void pom_fsevents_stop(FSEventStreamRef stream) {
	FSEventStreamStop(stream);
	FSEventStreamInvalidate(stream);
	FSEventStreamRelease(stream);
}

dispatch_queue_t pom_fsevents_queue(void) {
	return dispatch_queue_create("app.pomelo.fswatch", DISPATCH_QUEUE_SERIAL);
}

void pom_fsevents_release_queue(dispatch_queue_t queue) {
	dispatch_release(queue);
}
