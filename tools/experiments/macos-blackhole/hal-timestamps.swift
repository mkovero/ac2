// Logs Core Audio's own IOProc timestamps (mSampleTime, mHostTime, mRateScalar) on one
// device, input and output, beside the device's latency properties and the microphone
// authorization this process gets. The numbers cpal hides are what a direct Core Audio
// backend would index samples by.
//
//   swiftc -O hal-timestamps.swift -o hal-timestamps && ./hal-timestamps "BlackHole 2ch" 5
//
// It opens the device's IOProc and writes nothing to the output buffers (they stay as the
// HAL zeroed them).
import AVFoundation
import CoreAudio
import Foundation

let args = CommandLine.arguments
let want = args.count > 1 ? args[1] : "BlackHole 2ch"
let seconds = args.count > 2 ? Double(args[2]) ?? 5 : 5

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

var proc: AudioDeviceIOProcID? = nil
let queue = DispatchQueue(label: "hal-timestamps", qos: .userInteractive)
let st = AudioDeviceCreateIOProcIDWithBlock(&proc, dev, queue) { now, input, inTime, _, outTime in
    var peak: Float = 0
    var frames: UInt32 = 0
    for buf in UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: input)) {
        guard let d = buf.mData else { continue }
        let n = Int(buf.mDataByteSize) / MemoryLayout<Float>.size
        frames = UInt32(n) / max(buf.mNumberChannels, 1)
        let p = d.assumingMemoryBound(to: Float.self)
        for i in 0 ..< n { peak = max(peak, abs(p[i])) }
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
