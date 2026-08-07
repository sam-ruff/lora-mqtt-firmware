import 'package:flutter/foundation.dart';

import '../src/rust/frb_api.dart';
import '../src/rust/models.dart';
import 'platform_support.dart';

/// Central application state: owns the Rust engine and the last-known hub
/// snapshot.
class HubService extends ChangeNotifier {
  HubService({this.mockMode = false});

  /// Dev mode runs against the engine's in-memory fake hub, so the whole
  /// flow works with no hardware: flutter run --dart-define=MOCK=true
  final bool mockMode;

  static const String defaultHubAddress = 'http://192.168.4.1';

  bool _rustInitialised = false;
  bool connecting = false;
  bool saving = false;
  String hubAddress = defaultHubAddress;
  HubSnapshot? snapshot;
  Object? lastError;

  bool get connected => snapshot != null;

  Future<void> _ensureRustInitialised() async {
    if (_rustInitialised) return;
    await initRustLib();
    await frbInit(baseUrl: hubAddress, mock: mockMode);
    _rustInitialised = true;
  }

  /// Fetch config and status from the hub at [address].
  Future<bool> connect(String address) async {
    connecting = true;
    lastError = null;
    notifyListeners();
    try {
      await _ensureRustInitialised();
      if (address != hubAddress && !mockMode) {
        await frbSetHubAddress(baseUrl: address);
      }
      hubAddress = address;
      snapshot = await frbLoadSnapshot();
      return true;
    } catch (e) {
      lastError = e;
      snapshot = null;
      return false;
    } finally {
      connecting = false;
      notifyListeners();
    }
  }

  /// Refresh just the live status half of the snapshot.
  Future<void> refreshStatus() async {
    final current = snapshot;
    if (current == null) return;
    try {
      final status = await frbGetStatus();
      snapshot = HubSnapshot(config: current.config, status: status);
      notifyListeners();
    } catch (e) {
      lastError = e;
      notifyListeners();
    }
  }

  /// Validate and save an update. On success the hub restarts onto the
  /// configured network, so the session ends.
  Future<ApplyOutcome> save(ConfigUpdate update) async {
    saving = true;
    lastError = null;
    notifyListeners();
    try {
      return await frbSaveConfig(update: update);
    } catch (e) {
      lastError = e;
      rethrow;
    } finally {
      saving = false;
      notifyListeners();
    }
  }

  void disconnect() {
    snapshot = null;
    lastError = null;
    notifyListeners();
  }
}
