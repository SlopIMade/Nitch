plugins {
    id("com.android.application")
}

android {
    namespace = "app.nitch.tv"
    compileSdk = 37

    defaultConfig {
        applicationId = "app.nitch.tv"
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
