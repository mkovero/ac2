// Logs Core Audio's own IOProc timestamps (mSampleTime, mHostTime, mRateScalar) on one
// device, input and output, beside the device's latency properties and the microphone
// authorization this process gets. The numbers cpal hides are what a direct Core Audio
// backend would index samples by.
//
//   swiftc -O hal-timestamps.swift -o hal-timestamps
//   ./hal-timestamps "BlackHole 2ch" <seconds> [buffer frames, 0 = as is] [click]
//
// Without `click` it writes nothing to the output buffers (they stay as the HAL zeroed
// them). With `click` (CI runner on a virtual loopback driver only) it writes a -40 dBFS
// one-sample click every 24000 output samples and reports at which input sample time each
// returns: the loopback in the device's own sample-time domain.
import AVFoundation
import CoreAudio
import Foundation

// Line-buffered: a run killed while blocked in Core Audio still shows how far it got.
setvbuf(stdout, nil, _IOLBF, 0)
let args = CommandLine.arguments
let want = args.count > 1 ? args[1] : "BlackHole 2ch"
let seconds = args.count > 2 ? Double(args[2]) ?? 5 : 5
let wantBuffer = args.count > 3 ? UInt32(args[3]) ?? 0 : 0
let click = args.count > 4 && args[4] == "click"
if click, ProcessInfo.processInfo.environment["RUNNER_ENVIRONMENT"] != "github-hosted" || !want.hasPrefix("BlackHole") {
    print("click plays only into BlackHole on a GitHub-hosted runner")
    exit(2)
}

let auth = AVCaptureDevice.authorizationStatus(for: .audio)
let authName = ["notDetermined", "restricted", "denied", "authorized"]
print("AVCaptureDevice audio authorization: \(auth.rawValue) (\(auth.rawValue < 4 ? authName[auth.rawValue] : "?"))")

func address(_ sel: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(mSelector: sel, mScope: scope, mElement: kAudioObjectPropertyElementMain)
}

func u32(_ obj: AudioObjectID, _ sel: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope) -> String {
    var a = address(sel, scope)
    var v: UInt32 = 0
    var size = UInt32(MemoryLayout<UInt32>.size)
    let st = AudioObjectGetPropertyData(obj, &a, 0, nil, &size, &v)
    return st == noErr ? "\(v)" : "err \(st)"
}

func f64(_ obj: AudioObjectID, _ sel: AudioObjectPropertySelector) -> Double {
    var a = address(sel)
    var v: Float64 = 0
    var size = UInt32(MemoryLayout<Float64>.size)
    _ = AudioObjectGetPropertyData(obj, &a, 0, nil, &size, &v)
    return v
}

func name(_ obj: AudioObjectID) -> String {
    var a = address(kAudioObjectPropertyName)
    var v: Unmanaged<CFString>? = nil
    var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    let st = withUnsafeMutablePointer(to: &v) { AudioObjectGetPropertyData(obj, &a, 0, nil, &size, $0) }
    guard st == noErr, let s = v else { return "?" }
    return s.takeRetainedValue() as String
}

var devAddr = address(kAudioHardwarePropertyDevices)
var size: UInt32 = 0
let sys = AudioObjectID(kAudioObjectSystemObject)
AudioObjectGetPropertyDataSize(sys, &devAddr, 0, nil, &size)
var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
AudioObjectGetPropertyData(sys, &devAddr, 0, nil, &size, &ids)
var dev: AudioDeviceID = 0
for id in ids {
    let n = name(id)
    print("device \(id): \(n)")
    if n == want { dev = id }
}
guard dev != 0 else {
    print("no device named \(want)")
    exit(1)
}

if wantBuffer > 0 {
    var a = address(kAudioDevicePropertyBufferFrameSize)
    var v = wantBuffer
    let st = AudioObjectSetPropertyData(dev, &a, 0, nil, UInt32(MemoryLayout<UInt32>.size), &v)
    print("set buffer \(wantBuffer) frames: \(st)")
}
print("nominal rate \(f64(dev, kAudioDevicePropertyNominalSampleRate)) Hz")
for (label, scope) in [("input", kAudioObjectPropertyScopeInput), ("output", kAudioObjectPropertyScopeOutput)] {
    print("\(label): latency \(u32(dev, kAudioDevicePropertyLatency, scope)) safety offset \(u32(dev, kAudioDevicePropertySafetyOffset, scope)) buffer \(u32(dev, kAudioDevicePropertyBufferFrameSize, scope)) frames")
}

var tb = mach_timebase_info_data_t()
mach_timebase_info(&tb)
func ns(_ host: UInt64) -> Double { Double(host) * Double(tb.numer) / Double(tb.denom) }

struct Row {
    var nowSample: Float64, nowHost: UInt64
    var inSample: Float64, inHost: UInt64, inRate: Float64, inFlags: UInt32
    var outSample: Float64, outHost: UInt64
    var frames: UInt32, peak: Float
}
let capacity = 100_000
let rows = UnsafeMutablePointer<Row>.allocate(capacity: capacity)
var count = 0
let maxClicks = 64
let clickOut = UnsafeMutablePointer<Float64>.allocate(capacity: maxClicks)
let clickIn = UnsafeMutablePointer<Float64>.allocate(capacity: maxClicks)
var nOut = 0, nIn = 0
var nextClick: Float64 = -1
var lastIn: Float64 = -1e9

var proc: AudioDeviceIOProcID? = nil
let queue = DispatchQueue(label: "hal-timestamps", qos: .userInteractive)
let st = AudioDeviceCreateIOProcIDWithBlock(&proc, dev, queue) { now, input, inTime, output, outTime in
    var peak: Float = 0
    var frames: UInt32 = 0
    for buf in UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: input)) {
        guard let d = buf.mData else { continue }
        let n = Int(buf.mDataByteSize) / MemoryLayout<Float>.size
        let ch = Int(max(buf.mNumberChannels, 1))
        frames = UInt32(n / ch)
        let p = d.assumingMemoryBound(to: Float.self)
        for i in 0 ..< n {
            let x = abs(p[i])
            peak = max(peak, x)
            let t = inTime.pointee.mSampleTime + Double(i / ch)
            if click, x > 0.005, t > lastIn + 1000, nIn < maxClicks {
                clickIn[nIn] = t
                nIn += 1
                lastIn = t
            }
        }
    }
    if click {
        if nextClick < 0 { nextClick = outTime.pointee.mSampleTime + 4800 }
        for buf in UnsafeMutableAudioBufferListPointer(output) {
            guard let d = buf.mData else { continue }
            let ch = Int(max(buf.mNumberChannels, 1))
            let n = Int(buf.mDataByteSize) / MemoryLayout<Float>.size / ch
            let start = outTime.pointee.mSampleTime
            if nextClick >= start, nextClick < start + Double(n), nOut < maxClicks {
                let f = Int(nextClick - start)
                let p = d.assumingMemoryBound(to: Float.self)
                for c in 0 ..< ch { p[f * ch + c] = 0.01 }
                clickOut[nOut] = nextClick
                nOut += 1
                nextClick += 24000
            }
        }
    }
    if count < capacity {
        rows[count] = Row(
            nowSample: now.pointee.mSampleTime, nowHost: now.pointee.mHostTime,
            inSample: inTime.pointee.mSampleTime, inHost: inTime.pointee.mHostTime,
            inRate: inTime.pointee.mRateScalar, inFlags: inTime.pointee.mFlags.rawValue,
            outSample: outTime.pointee.mSampleTime, outHost: outTime.pointee.mHostTime,
            frames: frames, peak: peak)
        count += 1
    }
}
guard st == noErr, let proc else {
    print("AudioDeviceCreateIOProcIDWithBlock: \(st)")
    exit(1)
}
let started = AudioDeviceStart(dev, proc)
print("AudioDeviceStart: \(started)")
Thread.sleep(forTimeInterval: seconds)
AudioDeviceStop(dev, proc)
AudioDeviceDestroyIOProcID(dev, proc)
Thread.sleep(forTimeInterval: 0.2)

let n = count
print("callbacks: \(n)")
guard n > 2 else { exit(1) }
print("i now.sample now.host_ns in.sample in.host_ns in.rate in.flags out.sample out.host_ns frames peak")
for i in 0 ..< n where i < 12 || i >= n - 4 {
    let r = rows[i]
    print(String(format: "%d %.0f %.0f %.0f %.0f %.9f 0x%x %.0f %.0f %u %.6f", i, r.nowSample, ns(r.nowHost), r.inSample, ns(r.inHost), r.inRate, r.inFlags, r.outSample, ns(r.outHost), r.frames, r.peak))
}
// Sample-time gaps and the device rate against the host clock, from the input timestamps.
var gaps = 0
var maxPeak: Float = 0
for i in 0 ..< n {
    maxPeak = max(maxPeak, rows[i].peak)
    if i > 0, rows[i].inSample != rows[i - 1].inSample + Double(rows[i - 1].frames) { gaps += 1 }
}
let a = rows[1], b = rows[n - 1]
let rate = (b.inSample - a.inSample) / ((ns(b.inHost) - ns(a.inHost)) / 1e9)
print(String(format: "input sample-time gaps: %d; rate vs host clock: %.3f Hz; in→out sample offset: %.0f; out−in host: %.3f ms; now−in host: %.3f ms; max peak %.6f (%.1f dBFS)",
             gaps, rate, a.outSample - a.inSample, (ns(a.outHost) - ns(a.inHost)) / 1e6, (ns(a.nowHost) - ns(a.inHost)) / 1e6, maxPeak, maxPeak > 0 ? 20 * log10(Double(maxPeak)) : -Double.infinity))
if click {
    print("clicks out \(nOut), back \(nIn)")
    for i in 0 ..< min(nOut, nIn) {
        print(String(format: "click %d: out sample %.0f, in sample %.0f, loopback %.0f samples", i, clickOut[i], clickIn[i], clickIn[i] - clickOut[i]))
    }
}
