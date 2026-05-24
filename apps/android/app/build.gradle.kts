plugins {
    id("com.android.application")
}

android {
    namespace = "dev.androidconnect"
    compileSdk = 36
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "dev.androidconnect"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }
}

dependencies {
    // CameraX for live preview + image analysis during QR scan.
    implementation("androidx.camera:camera-camera2:1.3.4")
    implementation("androidx.camera:camera-lifecycle:1.3.4")
    implementation("androidx.camera:camera-view:1.3.4")
    // ML Kit barcode scanning powers the actual decode.
    implementation("com.google.mlkit:barcode-scanning:17.3.0")
}
