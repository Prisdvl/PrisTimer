buildscript {
    repositories {
        // 阿里云镜像置顶：google()/mavenCentral() 直连国内不稳（本构建环境代理
        // 对大文件限速易超时），镜像国内 CDN 直连且内容一致，兜底原始仓库。
        maven("https://maven.aliyun.com/repository/google")
        maven("https://maven.aliyun.com/repository/central")
        maven("https://maven.aliyun.com/repository/gradle-plugin")
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:8.11.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:1.9.25")
    }
}

allprojects {
    repositories {
        maven("https://maven.aliyun.com/repository/google")
        maven("https://maven.aliyun.com/repository/central")
        google()
        mavenCentral()
    }
}

tasks.register("clean").configure {
    delete("build")
}

