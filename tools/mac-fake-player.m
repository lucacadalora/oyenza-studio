// The Mac smoke run's stand-in for a browser tab or a music app (tools/mac-smoke.sh builds it into
// "Smoke Player.app"; the runner has no Spotify and no browser playing). It reports a song to macOS's
// Now Playing the way Safari, Chrome and Spotify do (MPNowPlayingInfoCenter), and takes play/pause,
// next, previous and seek from there (MPRemoteCommandCenter, where the island's buttons arrive through
// MediaRemote Adapter), writing each one as a line to the file named by its first argument. It plays
// no sound.
#import <AppKit/AppKit.h>
#import <MediaPlayer/MediaPlayer.h>

static NSString *gLog;
static BOOL gPlaying = YES;
static double gElapsed = 30.0;
static NSDate *gSince;

static void note(NSString *what) {
    NSString *line = [what stringByAppendingString:@"\n"];
    NSFileHandle *h = [NSFileHandle fileHandleForWritingAtPath:gLog];
    if (h == nil) {
        [line writeToFile:gLog atomically:NO encoding:NSUTF8StringEncoding error:nil];
        return;
    }
    [h seekToEndOfFile];
    [h writeData:[line dataUsingEncoding:NSUTF8StringEncoding]];
    [h closeFile];
}

static double elapsed(void) {
    return gPlaying ? gElapsed + [[NSDate date] timeIntervalSinceDate:gSince] : gElapsed;
}

static void publish(void) {
    MPNowPlayingInfoCenter *center = [MPNowPlayingInfoCenter defaultCenter];
    center.nowPlayingInfo = @{
        MPMediaItemPropertyTitle : @"Smoke Signals",
        MPMediaItemPropertyArtist : @"The Test Card",
        MPMediaItemPropertyAlbumTitle : @"Oyenza Studio CI",
        MPMediaItemPropertyPlaybackDuration : @245.0,
        MPNowPlayingInfoPropertyElapsedPlaybackTime : @(elapsed()),
        MPNowPlayingInfoPropertyPlaybackRate : @(gPlaying ? 1.0 : 0.0),
        MPNowPlayingInfoPropertyMediaType : @(MPNowPlayingInfoMediaTypeAudio),
    };
    center.playbackState = gPlaying ? MPNowPlayingPlaybackStatePlaying : MPNowPlayingPlaybackStatePaused;
}

static void setPlaying(BOOL playing) {
    gElapsed = elapsed();
    gSince = [NSDate date];
    gPlaying = playing;
    publish();
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        gLog = argc > 1 ? @(argv[1]) : @"/tmp/smoke-player.txt";
        gSince = [NSDate date];
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        MPRemoteCommandCenter *rc = [MPRemoteCommandCenter sharedCommandCenter];
        [rc.togglePlayPauseCommand addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *e) {
          note(@"toggle");
          setPlaying(!gPlaying);
          return MPRemoteCommandHandlerStatusSuccess;
        }];
        [rc.playCommand addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *e) {
          note(@"play");
          setPlaying(YES);
          return MPRemoteCommandHandlerStatusSuccess;
        }];
        [rc.pauseCommand addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *e) {
          note(@"pause");
          setPlaying(NO);
          return MPRemoteCommandHandlerStatusSuccess;
        }];
        [rc.nextTrackCommand addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *e) {
          note(@"next");
          return MPRemoteCommandHandlerStatusSuccess;
        }];
        [rc.previousTrackCommand addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *e) {
          note(@"previous");
          return MPRemoteCommandHandlerStatusSuccess;
        }];
        [rc.changePlaybackPositionCommand addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *e) {
          double t = ((MPChangePlaybackPositionCommandEvent *)e).positionTime;
          note([NSString stringWithFormat:@"seek %.1f", t]);
          gElapsed = t;
          gSince = [NSDate date];
          publish();
          return MPRemoteCommandHandlerStatusSuccess;
        }];
        publish();
        note(@"ready");
        [NSApp run];
    }
    return 0;
}
