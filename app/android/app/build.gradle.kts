plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "com.meshcore.walkie_textie_hub_config"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "com.meshcore.walkie_textie_hub_config"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    buildTypes {
        release {
            // TODO: Add your own signing config for the release build.
            // Signing with the debug keys for now, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

// --- Rust engine cross-compilation --------------------------------------------
// Builds librust_backend.so for each Android ABI with cargo-ndk and drops it into
// jniLibs, where flutter_rust_bridge loads it at runtime.
val rustProjectDir = File(rootProject.projectDir.parentFile, "rust_backend")
val jniLibsDir = file("src/main/jniLibs")
val androidAbis = listOf("arm64-v8a", "armeabi-v7a", "x86_64")
val cargoBin = File(System.getProperty("user.home"), ".cargo/bin")

val buildRustLib = tasks.register<Exec>("buildRustLib") {
    workingDir = rustProjectDir
    val ndkDir = android.ndkDirectory.absolutePath
    val home = System.getProperty("user.home")
    // Run cargo-ndk under a clean environment (`env -i`): Gradle's environment
    // otherwise leaks flags into the host build-script compilation, breaking the
    // link. We supply only HOME, PATH and the NDK location. cargo-ndk passes its
    // own --target, overriding the firmware repo's xtensa default.
    val cmd = mutableListOf(
        "/usr/bin/env", "-i",
        "HOME=$home",
        "PATH=${cargoBin.absolutePath}${File.pathSeparator}/usr/bin${File.pathSeparator}/bin",
        "ANDROID_NDK_HOME=$ndkDir",
        File(cargoBin, "cargo").absolutePath, "ndk",
    )
    androidAbis.forEach { cmd.add("-t"); cmd.add(it) }
    cmd.addAll(listOf("-o", jniLibsDir.absolutePath, "build", "--release"))
    commandLine(cmd)
}

tasks.named("preBuild") {
    dependsOn(buildRustLib)
}
