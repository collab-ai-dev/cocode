//! WebFetch preapproved URL matcher.
//!
//! Duplicates the WebFetch tool's preapproved host list to keep dependency
//! direction acyclic (`coco-tools` already depends on `coco-permissions`).

const PREAPPROVED_WEB_HOSTS: &[&str] = &[
    "platform.claude.com",
    "code.claude.com",
    "modelcontextprotocol.io",
    "github.com/anthropics",
    "agentskills.io",
    "docs.python.org",
    "en.cppreference.com",
    "docs.oracle.com",
    "learn.microsoft.com",
    "developer.mozilla.org",
    "go.dev",
    "pkg.go.dev",
    "www.php.net",
    "docs.swift.org",
    "kotlinlang.org",
    "ruby-doc.org",
    "doc.rust-lang.org",
    "www.typescriptlang.org",
    "react.dev",
    "angular.io",
    "vuejs.org",
    "nextjs.org",
    "expressjs.com",
    "nodejs.org",
    "bun.sh",
    "jquery.com",
    "getbootstrap.com",
    "tailwindcss.com",
    "d3js.org",
    "threejs.org",
    "redux.js.org",
    "webpack.js.org",
    "jestjs.io",
    "reactrouter.com",
    "docs.djangoproject.com",
    "flask.palletsprojects.com",
    "fastapi.tiangolo.com",
    "pandas.pydata.org",
    "numpy.org",
    "www.tensorflow.org",
    "pytorch.org",
    "scikit-learn.org",
    "matplotlib.org",
    "requests.readthedocs.io",
    "jupyter.org",
    "laravel.com",
    "symfony.com",
    "wordpress.org",
    "docs.spring.io",
    "hibernate.org",
    "tomcat.apache.org",
    "gradle.org",
    "maven.apache.org",
    "asp.net",
    "dotnet.microsoft.com",
    "nuget.org",
    "blazor.net",
    "reactnative.dev",
    "docs.flutter.dev",
    "developer.apple.com",
    "developer.android.com",
    "keras.io",
    "spark.apache.org",
    "huggingface.co",
    "www.kaggle.com",
    "www.mongodb.com",
    "redis.io",
    "www.postgresql.org",
    "dev.mysql.com",
    "www.sqlite.org",
    "graphql.org",
    "prisma.io",
    "docs.aws.amazon.com",
    "cloud.google.com",
    "kubernetes.io",
    "www.docker.com",
    "www.terraform.io",
    "www.ansible.com",
    "vercel.com/docs",
    "docs.netlify.com",
    "devcenter.heroku.com",
    "cypress.io",
    "selenium.dev",
    "docs.unity.com",
    "docs.unrealengine.com",
    "git-scm.com",
    "nginx.org",
    "httpd.apache.org",
];

pub(crate) fn is_preapproved_webfetch_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    is_preapproved_host(host, parsed.path())
}

fn is_preapproved_host(hostname: &str, pathname: &str) -> bool {
    let host_lower = hostname.to_ascii_lowercase();
    for entry in PREAPPROVED_WEB_HOSTS {
        match entry.split_once('/') {
            None if host_lower == *entry => return true,
            Some((host, path_rest)) if host_lower == host => {
                let prefix = format!("/{path_rest}");
                if pathname == prefix || pathname.starts_with(&format!("{prefix}/")) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
#[path = "web_preapproved.test.rs"]
mod tests;
