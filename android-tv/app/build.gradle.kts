plugins {
    id("com.android.application")
}

// Release builds (CI, on a version tag) pass the version as -PnitchVersion=1.2.3 and sign with the
// key from the environment (NITCH_KEYSTORE_FILE, NITCH_KEYSTORE_PASSWORD, NITCH_KEY_ALIAS,
// NITCH_KEY_PASSWORD; see scripts/create-apk-key.ps1). Without them: version 0.1.0, release
// builds unsigned.
val nitchVersion = (findProperty("nitchVersion") as String?) ?: "0.1.0"
val keystore: String? = System.getenv("NITCH_KEYSTORE_FILE")

/** 1.2.3 → 10203: always increasing with the version, as Android requires for updates. */
fun versionCodeOf(version: String): Int {
    val (major, minor, patch) = (version.split('-')[0].split('.').map { it.toIntOrNull() ?: 0 } + listOf(0, 0, 0)).take(3)
    return major * 10000 + minor * 100 + patch
}

android {
    namespace = "app.nitch.tv"
    compileSdk = 37

    defaultConfig {
        applicationId = "app.nitch.tv"
        minSdk = 26
        targetSdk = 37
        versionCode = maxOf(1, versionCodeOf(nitchVersion))
        versionName = nitchVersion
    }

    signingConfigs {
        if (keystore != null) {
            create("release") {
                storeFile = file(keystore)
                storePassword = System.getenv("NITCH_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("NITCH_KEY_ALIAS") ?: "nitch"
                keyPassword = System.getenv("NITCH_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            if (keystore != null) signingConfig = signingConfigs.getByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
