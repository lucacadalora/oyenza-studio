// The Mac smoke run's stand-in for a browser tab or a music app (tools/mac-smoke.sh builds it into
// "Smoke Player.app"; the runner has no Spotify and no browser playing). It reports a song to macOS's
// Now Playing the way Safari, Chrome and Spotify do (MPNowPlayingInfoCenter), and takes play/pause,
// next, previous and seek from there (MPRemoteCommandCenter, where the island's buttons arrive through
// MediaRemote Adapter), writing each one as a line to the file named by its first argument. While it
// plays it sends silence to the Mac's output, as a playing tab sends its sound: the island reads Now
// Playing only while something plays through the output.
#import <AVFoundation/AVFoundation.h>
#import <AppKit/AppKit.h>
#import <MediaPlayer/MediaPlayer.h>

static NSString *gLog;
static AVAudioEngine *gEngine;
static AVAudioPlayerNode *gNode;
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

// Silence through the output while playing; the output rests while paused.
static void sound(BOOL on) {
    if (on) {
        NSError *error = nil;
        if (![gEngine isRunning] && ![gEngine startAndReturnError:&error]) {
            note([NSString stringWithFormat:@"no sound: %@", error.localizedDescription]);
            return;
        }
        [gNode play];
    } else {
        [gNode pause];
        [gEngine pause];
    }
}

static void setPlaying(BOOL playing) {
    gElapsed = elapsed();
    gSince = [NSDate date];
    gPlaying = playing;
    sound(playing);
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
        gEngine = [[AVAudioEngine alloc] init];
        gNode = [[AVAudioPlayerNode alloc] init];
        AVAudioFormat *format = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:44100 channels:2];
        AVAudioPCMBuffer *silence = [[AVAudioPCMBuffer alloc] initWithPCMFormat:format frameCapacity:44100];
        silence.frameLength = 44100;
        for (AVAudioChannelCount c = 0; c < format.channelCount; c++) {
            memset(silence.floatChannelData[c], 0, sizeof(float) * silence.frameLength);
        }
        [gEngine attachNode:gNode];
        [gEngine connect:gNode to:gEngine.mainMixerNode format:format];
        [gNode scheduleBuffer:silence atTime:nil options:AVAudioPlayerNodeBufferLoops completionHandler:nil];
        sound(YES);
        publish();
        note(@"ready");
        [NSApp run];
    }
    return 0;
}
