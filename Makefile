.PHONY: restart build install

restart: install
	killall hyperforge 2>/dev/null || true
	sleep 1
	nohup hyperforge > /dev/null 2>&1 &
	sleep 2
	@echo "hyperforge restarted on port 44104"

build:
	cargo build

install: build
	cargo install --path .
