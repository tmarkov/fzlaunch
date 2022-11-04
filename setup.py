from setuptools import setup

setup(
    name="fzlaunch",
    version="1.0",
    scripts=["fzlaunch"],
    packages=["modules", "modules.lib"],
    package_data={"modules": ["modules/*"]},
    include_package_data=True,
)
